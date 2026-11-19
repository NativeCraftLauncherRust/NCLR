//! A transport the shell provides.
//!
//! On iOS the system networking already knows about proxies, VPNs, background
//! transfers and the trust store, so the app fetches and this library decides
//! what to fetch, checks every byte and writes it. With this transport the crate
//! builds for `aarch64-apple-ios` without `ureq`, `rustls` or `ring`, and so
//! without a C toolchain.
//!
//! The contract is one C function, called twice per URL:
//!
//! ```c
//! // Returns 0 and sets *length. With `out == NULL` it only reports the size.
//! int get(void *context, const char *url, unsigned char *out, size_t capacity, size_t *length);
//! ```
//!
//! Two calls keep ownership simple: the shell never hands over a buffer this
//! library would have to free.

use crate::fetch::Transport;
use std::ffi::{CString, c_char, c_void};

/// The shell's getter, as described above.
pub type GetFn = unsafe extern "C" fn(
    context: *mut c_void,
    url: *const c_char,
    out: *mut u8,
    capacity: usize,
    length: *mut usize,
) -> i32;

/// A [`Transport`] backed by the shell's networking.
pub struct CallbackTransport {
    context: *mut c_void,
    get: GetFn,
}

impl CallbackTransport {
    /// # Safety
    ///
    /// `context` must stay valid and `get` must be callable for as long as this
    /// transport is used, and both must tolerate calls from the thread that runs
    /// the download.
    pub unsafe fn new(context: *mut c_void, get: GetFn) -> CallbackTransport {
        CallbackTransport { context, get }
    }
}

impl Transport for CallbackTransport {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        let owned = CString::new(url).map_err(|_| format!("{url}: contains a NUL"))?;
        let mut length: usize = 0;
        let code = unsafe {
            (self.get)(
                self.context,
                owned.as_ptr(),
                std::ptr::null_mut(),
                0,
                &mut length,
            )
        };
        if code != 0 {
            return Err(format!("{url}: the shell refused ({code})"));
        }
        if length == 0 {
            return Err(format!("{url}: the shell reported no bytes"));
        }
        let mut buffer = vec![0u8; length];
        let mut written: usize = 0;
        let code = unsafe {
            (self.get)(
                self.context,
                owned.as_ptr(),
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut written,
            )
        };
        if code != 0 {
            return Err(format!("{url}: the shell refused ({code})"));
        }
        if written > buffer.len() {
            return Err(format!(
                "{url}: the shell wrote {written} bytes into {}",
                buffer.len()
            ));
        }
        buffer.truncate(written);
        Ok(buffer)
    }
}

impl std::fmt::Debug for CallbackTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CallbackTransport")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Serves from a table; the context is a pointer to it.
    unsafe extern "C" fn serve(
        context: *mut c_void,
        url: *const c_char,
        out: *mut u8,
        capacity: usize,
        length: *mut usize,
    ) -> i32 {
        let table = unsafe { &*(context as *const BTreeMap<String, Vec<u8>>) };
        let url = unsafe { CStr::from_ptr(url) }.to_string_lossy().into_owned();
        let Some(body) = table.get(&url) else {
            return 404;
        };
        unsafe { *length = body.len() };
        if out.is_null() {
            return 0;
        }
        if capacity < body.len() {
            return 413;
        }
        unsafe { std::ptr::copy_nonoverlapping(body.as_ptr(), out, body.len()) };
        0
    }

    static CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn counting(
        context: *mut c_void,
        url: *const c_char,
        out: *mut u8,
        capacity: usize,
        length: *mut usize,
    ) -> i32 {
        CALLS.fetch_add(1, Ordering::SeqCst);
        serve(context, url, out, capacity, length)
    }

    fn table() -> BTreeMap<String, Vec<u8>> {
        let mut table = BTreeMap::new();
        table.insert("https://example.invalid/a".to_string(), b"hello".to_vec());
        table
    }

    #[test]
    fn the_two_call_contract_returns_the_bytes() {
        let table = table();
        let transport = unsafe {
            CallbackTransport::new(
                &table as *const _ as *mut c_void,
                serve as GetFn,
            )
        };
        assert_eq!(
            transport.fetch("https://example.invalid/a").unwrap(),
            b"hello".to_vec()
        );
    }

    #[test]
    fn a_refusal_names_the_url_and_the_code() {
        let table = table();
        let transport = unsafe {
            CallbackTransport::new(&table as *const _ as *mut c_void, serve as GetFn)
        };
        let error = transport.fetch("https://example.invalid/missing").unwrap_err();
        assert!(error.contains("missing"), "{error}");
        assert!(error.contains("404"), "{error}");
    }

    #[test]
    fn each_url_costs_exactly_two_calls() {
        let table = table();
        CALLS.store(0, Ordering::SeqCst);
        let transport = unsafe {
            CallbackTransport::new(&table as *const _ as *mut c_void, counting as GetFn)
        };
        transport.fetch("https://example.invalid/a").unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn it_drives_the_real_download_loop_and_the_sha1_check() {
        use crate::fetch::download;
        use crate::plan::{Plan, RemoteFile};
        let body = b"the sha1 of these bytes must match".to_vec();
        let mut table = BTreeMap::new();
        table.insert("https://example.invalid/object".to_string(), body.clone());
        let transport = unsafe {
            CallbackTransport::new(&table as *const _ as *mut c_void, serve as GetFn)
        };

        let root = std::env::temp_dir().join(format!("hzr-callback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("object.bin");

        let sha1 = {
            use sha1::{Digest, Sha1};
            let mut hasher = Sha1::new();
            hasher.update(&body);
            format!("{:x}", hasher.finalize())
        };
        let plan = Plan {
            files: vec![RemoteFile {
                url: "https://example.invalid/object".to_string(),
                path: path.clone(),
                sha1: sha1.clone(),
                size: body.len() as u64,
                label: "object".to_string(),
            }],
        };
        let report = download(&transport, &plan, |_, _| {});
        assert!(report.is_clean(), "{:?}", report.failures);
        assert_eq!(std::fs::read(&path).unwrap(), body);

        // The same call again is skipped: the bytes are already right.
        let report = download(&transport, &plan, |_, _| {});
        assert!(report.is_clean());
        assert_eq!(report.bytes, 0, "it downloaded a second time");

        let _ = std::fs::remove_dir_all(&root);
    }
}
