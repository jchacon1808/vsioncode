#!/usr/bin/env python3
"""Oyente continuo de vsioncode.

Escucha el micrófono todo el rato, corta por frases y transcribe cada una.
Cada frase reconocida sale por la salida estándar como una línea JSON, que es
lo que vsioncode lee.

Se ejecuta con `uv run --with faster-whisper`, así que no hace falta instalar
nada en el sistema ni compilar nada.

## Por qué no vale medir el volumen

La primera versión decidía «hay voz» comparando la energía del sonido con un
umbral calibrado sobre el ruido de fondo. Es lo obvio, y falla en cuanto el
micrófono no es bueno.

Medido en un portátil corriente: el ruido de fondo daba 3388 de valor eficaz
con picos de 11873. Cualquier umbral por debajo del pico se dispara solo con
el ruido; cualquiera por encima se come la voz entera. No hay número que
funcione, porque el problema no es *cuánto* suena sino *qué* suena.

Así que la voz la detecta **Silero**, una red pequeña que ya viene con
faster-whisper y está entrenada justo para eso. No hay umbral que ajustar ni
que calibrar, y le da igual la ganancia del micrófono o el ventilador del
portátil.

## Por qué detectar la voz aquí y no dejárselo a whisper

Transcribir cuesta unos 2,5 segundos por cada 3 de audio en una máquina así.
Mandarle a whisper el silencio entre frases sería gastar ese tiempo en nada.

## Hablar como se habla

Al hablar de verdad uno hace pausas para pensar: «pues… estoy probando esto,
que no lo había probado… por aquí». Un oyente que corte al primer silencio
parte eso en cuatro mensajes.

Por eso hay dos silencios: uno corto que cierra un **trozo** —se transcribe y
se va viendo el texto crecer, pero no se manda nada— y otro largo que cierra
el **turno** y envía lo acumulado.

## El problema del eco

El altavoz y el micrófono están a diez centímetros. Si dejáramos el oyente
encendido mientras vsioncode habla, se transcribiría a sí mismo en bucle.
Mientras habla, vsioncode manda `pausa` por la entrada estándar y `sigue` al
terminar.

Protocolo, una línea JSON por mensaje:

    salida →  {"tipo":"listo"}                    ya se puede hablar
              {"tipo":"oyendo"}                   ha empezado a detectar voz
              {"tipo":"callado"}                  ha dejado de detectarla
              {"tipo":"parcial","texto":"..."}    un trozo, para irlo enseñando
              {"tipo":"fin_turno"}                ha terminado: enviar lo acumulado
              {"tipo":"error","texto":"..."}      algo fue mal
    entrada ←  pausa | sigue | fin
"""

import array
import json
import os
import subprocess
import sys
import threading
import wave

TASA = 16000          # whisper y Silero trabajan a 16 kHz
CANALES = 1
BYTES_MUESTRA = 2     # S16_LE

# Silero mira el audio en trozos de 512 muestras. No es negociable: la red
# está entrenada con ese tamaño.
MUESTRAS_TROZO = 512
BYTES_TROZO = MUESTRAS_TROZO * BYTES_MUESTRA
MS_TROZO = MUESTRAS_TROZO * 1000 / TASA          # 32 ms

# Cuántos trozos se le pasan a la red de una vez. Con 8 son 256 ms de retraso
# en decidir, que no se nota, y evita llamarla 31 veces por segundo.
TROZOS_POR_TANDA = 8

# Por encima de esta probabilidad, la red dice que eso es voz. 0,5 es el valor
# con el que viene entrenada; subirlo se come los principios de frase.
PROB_VOZ = 0.5

# Dos silencios, y la diferencia entre ellos es lo que permite hablar con
# pausas: el corto cierra un trozo, el largo cierra el turno.
MS_PAUSA = 700
TROZOS_PAUSA = int(MS_PAUSA / MS_TROZO)
MS_FIN_TURNO = 2500
TROZOS_FIN_TURNO = int(MS_FIN_TURNO / MS_TROZO)

# Cuánta voz seguida hace falta para dar por empezada una frase. Filtra una tos
# o un portazo que la red confunda un instante.
MS_VOZ_MINIMA = 160
TROZOS_VOZ = max(1, int(MS_VOZ_MINIMA / MS_TROZO))

# Nada más corto que esto se transcribe: sería ruido.
MS_FRASE_MINIMA = 400
# Y nada más largo: si algo se queda abierto, no puede crecer sin fin.
SEG_FRASE_MAXIMA = 30

# Al abrir el micrófono, `arecord` suelta un chasquido: unas pocas muestras al
# máximo del rango que no son sonido, son el arranque del dispositivo. Medir
# ahí llevaba a bajar la ganancia una y otra vez creyendo que saturaba, hasta
# dejar el micrófono tan bajo que ya no se oía a nadie hablar.
SEG_DESCARTE_INICIAL = 0.75

# El micrófono está recortando cuando la onda ya no cabe en el rango y se le
# corta la punta. Esa información no vuelve.
NIVEL_RECORTE = 32700

# Nivel demasiado bajo: por debajo de esto la voz se pierde en el ruido de
# cuantificación y el reconocedor no tiene con qué trabajar. Hay que SUBIR.
PICO_MINIMO = 2500

# Y no basta con «que ahora no recorte»: hay que dejar sitio para la voz.
#
# En reposo, un micrófono bien puesto se queda muy por debajo del máximo,
# porque hablar es bastante más fuerte que el ruido de una habitación. Si el
# ruido ya llega al 40% del rango, la voz se sale seguro. Se ajusta mirando el
# **pico en reposo**, no cuánto recorta ahora: para cuando recorta, ya es
# tarde.
PICO_OBJETIVO = 12000

# Cuánto recorte hace falta para dar por saturado el micrófono.
#
# Cuidado con afinar esto a la baja. Con la máquina cargada, `arecord` pierde
# muestras del buffer y aparecen picos al máximo que **no son saturación**: son
# huecos. Medido en este equipo, con carga 10 el pico llegaba a 32768 y con
# carga 2 se quedaba en 13737, con el mismo micrófono y la misma ganancia.
#
# Un umbral demasiado fino convierte eso en una espiral: se baja la ganancia
# por un artefacto de CPU, la voz se queda inaudible, y el usuario habla sin
# que pase nada. Pasó.
FRACCION_RECORTE = 0.005


def avisar(**campos):
    sys.stdout.write(json.dumps(campos, ensure_ascii=False) + "\n")
    sys.stdout.flush()


class Oyente:
    def __init__(self, modelo_nombre: str, dispositivo: str | None):
        self.pausado = threading.Event()
        self.fin = threading.Event()
        self.modelo_nombre = modelo_nombre
        self.dispositivo = dispositivo
        self.modelo = None
        self.vad = None
        self.np = None

    # ---------------------------------------------------------------- control

    def escuchar_ordenes(self):
        """Lee `pausa`, `sigue` y `fin` de la entrada estándar."""
        for linea in sys.stdin:
            orden = linea.strip().lower()
            if orden == "pausa":
                self.pausado.set()
            elif orden == "sigue":
                self.pausado.clear()
            elif orden == "fin":
                self.fin.set()
                return

    # ---------------------------------------------------------------- modelos

    def cargar(self):
        import numpy as np
        from faster_whisper import WhisperModel
        from faster_whisper.vad import get_vad_model

        self.np = np
        # int8 en CPU: es lo que hace esto viable sin tarjeta gráfica.
        self.modelo = WhisperModel(
            self.modelo_nombre,
            device="cpu",
            compute_type="int8",
            cpu_threads=os.cpu_count() or 4,
        )
        self.vad = get_vad_model()

    def probabilidades(self, pcm: bytes):
        """Probabilidad de voz de cada trozo de 512 muestras."""
        muestras = self.np.frombuffer(pcm, dtype="<i2").astype("float32") / 32768.0
        return self.np.asarray(self.vad(muestras)).reshape(-1)

    def transcribir(self, pcm: bytes) -> str:
        import tempfile

        with tempfile.NamedTemporaryFile(suffix=".wav", delete=True) as f:
            with wave.open(f.name, "wb") as w:
                w.setnchannels(CANALES)
                w.setsampwidth(BYTES_MUESTRA)
                w.setframerate(TASA)
                w.writeframes(pcm)
            # `beam_size=1` es búsqueda voraz: bastante peor para un texto largo
            # y casi igual para una frase suelta, a cambio de ir al doble de
            # velocidad. Aquí siempre son frases sueltas.
            segmentos, _ = self.modelo.transcribe(
                f.name,
                language="es",
                beam_size=1,
                condition_on_previous_text=False,
            )
            return " ".join(s.text.strip() for s in segmentos).strip()

    # ------------------------------------------------------------------ audio

    def abrir_microfono(self):
        # Entrada de fichero para poder probar la cadena entera —detección de
        # voz incluida— sin que haya un humano hablando. Con un micrófono de
        # verdad no se usa nunca.
        fichero = os.environ.get("VSION_AUDIO_FICHERO")
        if fichero:
            # A ritmo real, no a velocidad de disco: si volcáramos el fichero de
            # golpe, la prueba no se parecería en nada a un micrófono.
            lector = (
                "import sys,time\n"
                f"f=open({fichero!r},'rb')\n"
                f"n={BYTES_TROZO}\n"
                "t=time.time()\n"
                "while True:\n"
                "    b=f.read(n)\n"
                "    if not b: break\n"
                "    sys.stdout.buffer.write(b); sys.stdout.buffer.flush()\n"
                f"    t+={MS_TROZO / 1000}\n"
                "    d=t-time.time()\n"
                "    if d>0: time.sleep(d)\n"
            )
            return subprocess.Popen(
                [sys.executable, "-c", lector],
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
            )
        orden = ["arecord", "-q", "-t", "raw", "-f", "S16_LE", "-r", str(TASA), "-c", str(CANALES)]
        if self.dispositivo:
            orden += ["-D", self.dispositivo]
        return subprocess.Popen(orden, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)

    def medir(self, pcm: bytes) -> tuple[int, float]:
        """Pico y fracción recortada de una muestra."""
        m = array.array("h")
        m.frombytes(pcm[: len(pcm) // 2 * 2])
        if not m:
            return (0, 0.0)
        pico = max(abs(x) for x in m)
        recortadas = sum(1 for x in m if abs(x) >= NIVEL_RECORTE) / len(m)
        return (pico, recortadas)

    def poner_ganancia(self, nivel: int) -> bool:
        try:
            subprocess.run(
                ["amixer", "-c", "0", "-q", "set", "Capture", f"{nivel}%"],
                check=False,
                timeout=5,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            return True
        except Exception:  # noqa: BLE001
            return False

    def bien_ajustado(self, pcm: bytes) -> bool:
        pico, recortadas = self.medir(pcm)
        return pico <= PICO_OBJETIVO and recortadas <= FRACCION_RECORTE

    def descartar_transitorio(self, salida):
        """Tira el chasquido de arranque de `arecord`."""
        n = int(SEG_DESCARTE_INICIAL * TASA) * BYTES_MUESTRA
        n = (n // BYTES_TROZO) * BYTES_TROZO
        self.leer_exacto(salida, n)

    def ajustar_ganancia(self, salida) -> str | None:
        """Baja la ganancia de captura hasta que el micrófono deje de recortar.

        No basta con avisar al usuario del comando: pipewire restaura el nivel
        por su cuenta, así que el aviso reaparece cada sesión y el problema
        nunca se arregla del todo.

        Y arreglarlo importa más de lo que parece. Recortar no es «sonar
        fuerte»: es que la onda ya no cabe en el rango y se le corta la punta.
        Esa información no se recupera después, así que el reconocedor recibe
        una señal destrozada y transcribe cualquier cosa. Todo lo que venga
        detrás —detección de voz, transcripción, respuesta— depende de esto.
        """
        self.descartar_transitorio(salida)
        muestra = self.leer_exacto(salida, BYTES_TROZO * TROZOS_POR_TANDA * 8)
        if not muestra:
            return "el micrófono no dio ningún dato"

        pico, _ = self.medir(muestra)
        # Demasiado bajo también es un problema, y hasta ahora solo se
        # contemplaba lo contrario: la ganancia solo sabía bajar, así que un
        # micrófono flojo se quedaba flojo para siempre.
        if pico < PICO_MINIMO:
            for nivel in (60, 80, 100):
                self.poner_ganancia(nivel)
                self.descartar_transitorio(salida)
                muestra = self.leer_exacto(salida, BYTES_TROZO * TROZOS_POR_TANDA * 4)
                p, _ = self.medir(muestra)
                if p >= PICO_MINIMO:
                    return f"tu micrófono venía muy bajo; le he subido la ganancia al {nivel}%"
            return "tu micrófono suena muy bajo incluso al máximo de ganancia"

        if self.bien_ajustado(muestra):
            return None

        for nivel in (60, 45, 35, 25):
            if not self.poner_ganancia(nivel):
                return (
                    "el micrófono satura y no pude tocar la ganancia; prueba "
                    "«amixer -c 0 set Capture 40%»"
                )
            # Se descarta el audio de la transición y se vuelve a medir.
            self.descartar_transitorio(salida)
            muestra = self.leer_exacto(salida, BYTES_TROZO * TROZOS_POR_TANDA * 4)
            if self.bien_ajustado(muestra):
                return f"tu micrófono venía demasiado alto; le he bajado la ganancia al {nivel}%"

        return (
            "el micrófono sigue demasiado alto incluso al 12% de ganancia; "
            "puede que tengas una fuente de ruido muy cerca"
        )

    def leer_exacto(self, salida, n: int) -> bytes:
        """Lee exactamente `n` bytes.

        Una lectura corta descuadraría los trozos de 512 muestras que la red
        espera, y a partir de ahí todas las decisiones irían desplazadas.
        """
        buf = bytearray()
        while len(buf) < n:
            t = salida.read(n - len(buf))
            if not t:
                return bytes(buf)
            buf.extend(t)
        return bytes(buf)

    # ------------------------------------------------------------------ bucle

    def correr(self):
        self.cargar()
        mic = self.abrir_microfono()
        salida = mic.stdout

        # No hay que calibrar ningún umbral —Silero no los usa—, pero sí hay
        # que asegurarse de que lo que entra no viene ya destrozado.
        avisar(
            tipo="listo",
            modelo=self.modelo_nombre,
            aviso=self.ajustar_ganancia(salida),
        )

        frase = bytearray()
        trozos_voz = 0
        trozos_silencio = 0
        hablando = False
        turno_abierto = False
        max_bytes = SEG_FRASE_MAXIMA * TASA * BYTES_MUESTRA
        bytes_tanda = BYTES_TROZO * TROZOS_POR_TANDA

        while not self.fin.is_set():
            tanda = self.leer_exacto(salida, bytes_tanda)
            if len(tanda) < bytes_tanda:
                break

            # En pausa se sigue leyendo y tirando: si dejáramos de leer, el
            # buffer del micrófono se llenaría y al reanudar oiríamos lo de
            # hace medio minuto.
            if self.pausado.is_set():
                if hablando:
                    avisar(tipo="callado")
                hablando, frase, trozos_voz = False, bytearray(), 0
                continue

            try:
                probs = self.probabilidades(tanda)
            except Exception as e:  # noqa: BLE001
                avisar(tipo="error", texto=f"detección de voz: {e}")
                break

            for i, p in enumerate(probs):
                trozo = tanda[i * BYTES_TROZO : (i + 1) * BYTES_TROZO]
                hay_voz = float(p) >= PROB_VOZ

                if not hablando:
                    trozos_silencio += 1
                    # Silencio largo con algo ya dicho: has terminado de hablar.
                    if turno_abierto and trozos_silencio >= TROZOS_FIN_TURNO:
                        turno_abierto = False
                        avisar(tipo="fin_turno")

                    if hay_voz:
                        trozos_voz += 1
                        frase.extend(trozo)
                        if trozos_voz >= TROZOS_VOZ:
                            hablando = True
                            trozos_silencio = 0
                            avisar(tipo="oyendo")
                    else:
                        # Un trozo suelto con voz no arranca nada, pero se
                        # guarda algo de contexto para no comerse la primera
                        # sílaba.
                        trozos_voz = 0
                        frase = bytearray(trozo)
                    continue

                frase.extend(trozo)
                trozos_silencio = 0 if hay_voz else trozos_silencio + 1

                if trozos_silencio >= TROZOS_PAUSA or len(frase) >= max_bytes:
                    pcm = bytes(frase)
                    frase, trozos_voz, hablando = bytearray(), 0, False
                    # Se avisa siempre, aunque el trozo se descarte por corto.
                    # Sin esto, un ruido que arranca «oyendo» y no llega a
                    # frase dejaba el indicador encendido para siempre.
                    avisar(tipo="callado")

                    if len(pcm) < MS_FRASE_MINIMA * TASA * BYTES_MUESTRA // 1000:
                        continue
                    try:
                        texto = self.transcribir(pcm)
                    except Exception as e:  # noqa: BLE001
                        avisar(tipo="error", texto=str(e))
                        continue
                    if texto and len(texto) > 1:
                        turno_abierto = True
                        avisar(tipo="parcial", texto=texto)

        mic.terminate()


def main():
    modelo = sys.argv[1] if len(sys.argv) > 1 else "tiny"
    dispositivo = sys.argv[2] if len(sys.argv) > 2 else None
    oyente = Oyente(modelo, dispositivo)
    threading.Thread(target=oyente.escuchar_ordenes, daemon=True).start()
    try:
        oyente.correr()
    except KeyboardInterrupt:
        pass
    except Exception as e:  # noqa: BLE001
        avisar(tipo="error", texto=str(e))
        sys.exit(1)


if __name__ == "__main__":
    main()
