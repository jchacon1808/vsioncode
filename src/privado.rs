//! Cerrar un fichero o una carpeta a quien lo creó, en los dos sistemas.
//!
//! Existe porque la misma garantía —«esto solo lo lee su dueño»— se escribe de
//! dos formas que no se parecen en nada: en Unix son tres bits de modo, y en
//! Windows es una lista de control de acceso que además hay que **desheredar**
//! del directorio de arriba. Repartir esa diferencia por los sitios que
//! guardan claves era pedir que uno de ellos se quedara a medias.
//!
//! Aquí solo hay dos verbos, y el segundo es la mitad importante:
//!
//! - [`cerrar`] aprieta lo que ya existe.
//! - [`esta_cerrado`] lo comprueba **leyéndolo de vuelta**. Un `.env` que
//!   parece protegido y no lo está es peor que uno que se sabe abierto, así
//!   que la prueba no puede consistir en confiar en que la llamada anterior
//!   hizo lo que dijo.

use std::io;
use std::path::Path;

/// Deja `ruta` accesible solo para el usuario actual.
///
/// Sirve para ficheros y para carpetas: en Unix cambia el modo —600 o 700
/// según lo que sea— y en Windows pone una ACL de una sola entrada, sin
/// herencia.
pub fn cerrar(ruta: &Path) -> io::Result<()> {
    imp::cerrar(ruta)
}

/// ¿Está de verdad cerrado a su dueño?
///
/// Se lee del sistema, no de lo que creemos haber escrito.
pub fn esta_cerrado(ruta: &Path) -> io::Result<bool> {
    imp::esta_cerrado(ruta)
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub fn cerrar(ruta: &Path) -> io::Result<()> {
        let modo = if ruta.is_dir() { 0o700 } else { 0o600 };
        std::fs::set_permissions(ruta, std::fs::Permissions::from_mode(modo))
    }

    pub fn esta_cerrado(ruta: &Path) -> io::Result<bool> {
        let modo = std::fs::metadata(ruta)?.permissions().mode() & 0o777;
        // Lo que importa es que no haya nada para el grupo ni para los demás.
        // Que el dueño lo tenga a 600 o a 400 es asunto suyo.
        Ok(modo & 0o077 == 0)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{CloseHandle, HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::EXPLICIT_ACCESS_W;
    use windows_sys::Win32::Security::Authorization::{
        GetNamedSecurityInfoW, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW, SetNamedSecurityInfoW,
        TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetTokenInformation,
        PROTECTED_DACL_SECURITY_INFORMATION, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// El SID del usuario que corre este proceso, con su memoria detrás.
    ///
    /// El `Vec` no es decoración: `PSID` apunta dentro de él, así que mientras
    /// se use el puntero el buffer tiene que seguir vivo. Devolver solo el
    /// puntero es la forma exacta de escribir un uso después de liberar.
    struct Dueño {
        buffer: Vec<u8>,
    }

    impl Dueño {
        fn sid(&self) -> PSID {
            // El SID vive al final de la estructura, apuntado por `User.Sid`.
            unsafe { (*(self.buffer.as_ptr() as *const TOKEN_USER)).User.Sid }
        }
    }

    fn dueño() -> io::Result<Dueño> {
        unsafe {
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }

            // Dos llamadas, como en el registro: la primera dice cuánto ocupa.
            let mut hacen_falta = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut hacen_falta);
            if hacen_falta == 0 {
                let e = io::Error::last_os_error();
                CloseHandle(token);
                return Err(e);
            }

            let mut buffer = vec![0u8; hacen_falta as usize];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                hacen_falta,
                &mut hacen_falta,
            );
            let e = io::Error::last_os_error();
            CloseHandle(token);
            if ok == 0 {
                return Err(e);
            }
            Ok(Dueño { buffer })
        }
    }

    fn ancho(ruta: &Path) -> Vec<u16> {
        ruta.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Una ACL de una sola entrada, y **protegida**.
    ///
    /// Lo de protegida es la mitad del trabajo y la que se olvida: sin
    /// `PROTECTED_DACL_SECURITY_INFORMATION` la lista que se pone aquí se suma
    /// a la que baja heredada del directorio, así que el `.env` seguiría
    /// siendo legible por todo el que pudiera leer la carpeta de arriba —y la
    /// llamada habría devuelto éxito—.
    pub fn cerrar(ruta: &Path) -> io::Result<()> {
        let d = dueño()?;
        let nombre = ancho(ruta);

        // Una carpeta reparte lo suyo a lo que nazca dentro; un fichero no
        // tiene a quién repartírselo.
        let herencia = if ruta.is_dir() {
            windows_sys::Win32::Security::SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            windows_sys::Win32::Security::NO_INHERITANCE
        };

        let ea = EXPLICIT_ACCESS_W {
            // Control total: es el dueño, y el equivalente del 600 no es «leer»
            // sino «todo para mí y nada para nadie».
            grfAccessPermissions: windows_sys::Win32::Foundation::GENERIC_ALL,
            grfAccessMode: SET_ACCESS,
            grfInheritance: herencia,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: d.sid().cast(),
            },
        };

        unsafe {
            let mut acl: *mut ACL = std::ptr::null_mut();
            let r = SetEntriesInAclW(1, &ea, std::ptr::null(), &mut acl);
            if r != 0 {
                return Err(io::Error::from_raw_os_error(r as i32));
            }

            let r = SetNamedSecurityInfoW(
                nombre.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null(),
            );
            LocalFree(acl as HLOCAL);
            if r != 0 {
                return Err(io::Error::from_raw_os_error(r as i32));
            }
        }
        Ok(())
    }

    /// Se lee la ACL de vuelta y se cuenta: una entrada, y para nosotros.
    ///
    /// Contar es tan importante como mirar de quién es. Una lista con nuestra
    /// entrada la primera y `Usuarios` la segunda pasaría cualquier
    /// comprobación que se conforme con encontrarse a sí misma.
    pub fn esta_cerrado(ruta: &Path) -> io::Result<bool> {
        let d = dueño()?;
        let nombre = ancho(ruta);

        unsafe {
            let mut acl: *mut ACL = std::ptr::null_mut();
            let mut descriptor = std::ptr::null_mut();
            let r = GetNamedSecurityInfoW(
                nombre.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut descriptor,
            );
            if r != 0 {
                return Err(io::Error::from_raw_os_error(r as i32));
            }

            // Una DACL nula no es una lista vacía: quiere decir «acceso para
            // todo el mundo», que es el peor caso. Y una lista con más de una
            // entrada tiene a alguien más dentro. Las dos responden que no.
            let cerrado = if acl.is_null() || (*acl).AceCount != 1 {
                false
            } else {
                let mut ace = std::ptr::null_mut();
                if GetAce(acl, 0, &mut ace) == 0 {
                    false
                } else {
                    let ace = ace as *const ACCESS_ALLOWED_ACE;
                    // El SID empieza donde declara `SidStart` y sigue de ahí.
                    let sid = std::ptr::addr_of!((*ace).SidStart) as PSID;
                    EqualSid(sid, d.sid()) != 0
                }
            };

            LocalFree(descriptor as HLOCAL);
            Ok(cerrado)
        }
    }
}
