#!/usr/bin/env python3
"""Diagnóstico del micrófono.

Graba unos segundos y dice, con números, si el micrófono sirve para hablarle
al agente. Existe porque «no me oye» no es un dato: puede ser la ganancia, el
ruido de fondo, el dispositivo equivocado o que la voz llegue demasiado floja,
y cada causa se arregla de una forma distinta.

Lo importante no es el volumen, es la **diferencia** entre cuando hablas y
cuando callas. Un micrófono con mucho ruido puede sonar muy alto y aun así ser
inservible, porque la voz no sobresale.
"""

import array
import json
import math
import os
import subprocess
import sys

TASA = 16000
BYTES_MUESTRA = 2
MUESTRAS_TROZO = 512
BYTES_TROZO = MUESTRAS_TROZO * BYTES_MUESTRA
SEG_DESCARTE = 0.75          # el chasquido de arranque de arecord


def rms(t):
    if not t:
        return 0.0
    return math.sqrt(sum(float(x) * x for x in t) / len(t))


def main():
    segundos = float(sys.argv[1]) if len(sys.argv) > 1 else 5.0
    orden = ["arecord", "-q", "-t", "raw", "-f", "S16_LE", "-r", str(TASA), "-c", "1",
             "-d", str(int(segundos + SEG_DESCARTE + 1))]
    try:
        cruda = subprocess.run(orden, capture_output=True, timeout=segundos + 12).stdout
    except Exception as e:  # noqa: BLE001
        print(json.dumps({"error": f"no pude grabar: {e}"}, ensure_ascii=False))
        return

    saltar = int(SEG_DESCARTE * TASA) * BYTES_MUESTRA
    d = array.array("h")
    d.frombytes(cruda[saltar : len(cruda) // 2 * 2])
    if not d:
        print(json.dumps({"error": "el micrófono no dio nada"}, ensure_ascii=False))
        return

    n = MUESTRAS_TROZO
    tramos = [d[i : i + n] for i in range(0, len(d) - n, n)]
    energias = sorted(rms(t) for t in tramos)
    fondo = energias[len(energias) // 10]          # percentil 10: el silencio
    fuerte = energias[len(energias) * 9 // 10]     # percentil 90: lo más alto
    pico = max(abs(x) for x in d)
    recortadas = sum(1 for x in d if abs(x) >= 32700) / len(d)

    # ¿Encuentra voz Silero? Es la prueba que de verdad importa: es quien lo
    # decide en marcha.
    trozos_voz, total = 0, 0
    try:
        import numpy as np
        from faster_whisper.vad import get_vad_model

        vad = get_vad_model()
        m = np.frombuffer(d.tobytes(), dtype="<i2").astype("float32") / 32768.0
        m = m[: len(m) // MUESTRAS_TROZO * MUESTRAS_TROZO]
        p = np.asarray(vad(m)).reshape(-1)
        total = len(p)
        trozos_voz = int((p >= 0.5).sum())
    except Exception:  # noqa: BLE001
        pass

    relacion = 20 * math.log10(fuerte / fondo) if fondo > 1 and fuerte > fondo else 0.0

    print(json.dumps({
        "fondo": round(fondo),
        "fuerte": round(fuerte),
        "pico": pico,
        "recortado_pct": round(recortadas * 100, 2),
        "relacion_db": round(relacion, 1),
        "trozos_con_voz": trozos_voz,
        "trozos_totales": total,
    }, ensure_ascii=False))


if __name__ == "__main__":
    main()
