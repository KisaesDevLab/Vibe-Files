//! Windows DPAPI (QUESTIONS Q-38; the "remember on this PC" part of PA-15): bytes sealed for the
//! current Windows login on this computer. Another login, or another computer, cannot open them.
//! `entropy` binds a blob to its purpose (e.g. the cabinet id).

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: b.len() as u32,
        pbData: b.as_ptr() as *mut u8,
    }
}

/// Copies the output blob and frees it.
///
/// SAFETY: `out` was filled by a successful DPAPI call (LocalAlloc'd buffer of `cbData` bytes).
unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    unsafe {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut core::ffi::c_void)));
        v
    }
}

/// Seals `data` for this Windows login on this computer.
pub fn protect(data: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
    let (inp, ent) = (blob(data), blob(entropy));
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the input blobs point at live slices for the duration of the call.
    unsafe {
        CryptProtectData(
            &inp,
            windows::core::w!("Vibe Files"),
            Some(&ent),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(|e| format!("DPAPI could not seal: {e}"))?;
        Ok(take(out))
    }
}

/// Opens a blob from [`protect`] (fails for another login, computer or `entropy`).
pub fn unprotect(sealed: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
    let (inp, ent) = (blob(sealed), blob(entropy));
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    unsafe {
        CryptUnprotectData(
            &inp,
            None,
            Some(&ent),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(|e| format!("DPAPI could not open: {e}"))?;
        Ok(take(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_wrong_entropy() {
        let sealed = protect(b"firm key bytes", b"cab-1").unwrap();
        assert!(!sealed.windows(4).any(|w| w == b"firm"));
        assert_eq!(unprotect(&sealed, b"cab-1").unwrap(), b"firm key bytes");
        assert!(unprotect(&sealed, b"cab-2").is_err());
    }
}
