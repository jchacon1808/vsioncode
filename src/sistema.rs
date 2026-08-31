//! Las preguntas al sistema operativo que `std` no sabe responder.
//!
//! Son pocas y siempre las mismas: cosas que en Linux se leen de `/proc` y en
//! Windows hay que ir a pedir a una API. Viven juntas porque **estaban
//! duplicadas y eso ya costó un fallo**: `pty.rs` y `adjunto.rs` tenían cada
//! uno su «¿sigue vivo este proceso?», se arregló el de `pty.rs` para Windows
//! y el otro se quedó mintiendo. El resultado era que las capturas pegadas no
//! se barrían nunca y el directorio temporal crecía sin fin.
//!
//! Una copia de algo así no se detecta sola: las dos compilan, las dos pasan
//! sus pruebas, y la que se quedó atrás no falla —simplemente contesta que sí
//! a todo—.

/// ¿Existe todavía ese proceso?
///
/// Donde no se puede saber, la respuesta es que sí: equivocarse por exceso deja
/// un fichero de sobra en un directorio temporal, y equivocarse por defecto le
/// borra el suyo a algo que está en uso.
#[cfg(target_os = "linux")]
pub fn vivo(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// En los Unix sin `/proc` —macOS, los BSD— no hay nada barato que mirar, así
/// que se contesta que sí y se deja la basura.
#[cfg(all(unix, not(target_os = "linux")))]
pub fn vivo(_pid: u32) -> bool {
    true
}

/// En Windows sí se puede saber: si `OpenProcess` con el permiso más pequeño
/// que existe devuelve un manejador, el proceso está.
///
/// Se pide `PROCESS_QUERY_LIMITED_INFORMATION` a propósito: es el único que
/// concede el sistema sobre un proceso de otra sesión o de otro usuario, y aquí
/// no hace falta nada más que su existencia.
#[cfg(windows)]
pub fn vivo(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        // Puede ser que no exista o que no nos dejen mirar. Se responde que sí
        // por lo mismo de siempre: el error caro es borrar el fichero de algo
        // que sigue trabajando.
        return unsafe { windows_sys::Win32::Foundation::GetLastError() }
            != windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER;
    }
    unsafe { CloseHandle(h) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El proceso que hace la pregunta está vivo por definición.
    #[test]
    fn uno_mismo_siempre_esta() {
        assert!(vivo(std::process::id()));
    }

    /// Y uno imposible, no. En Linux 4194304 es el tope de PID; en Windows los
    /// identificadores son múltiplos de cuatro, así que ese valor tampoco es
    /// de nadie.
    #[test]
    fn uno_que_no_existe_no_esta() {
        assert!(!vivo(4194304));
    }
}
