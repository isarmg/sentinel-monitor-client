//! Foundation revision 1 owns all buffers, handles and panic boundaries.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use std::sync::{Arc, OnceLock};
use tokio::runtime::Runtime;
use uuid::Uuid;
use xcoc::{
    mobile::{self, CaptureMetadata, MobileClient, Pairing},
    rtsp_publish::MAX_FRAME_BYTES,
};
use xcsc_mobile_ffi::{self as ffi, FfiError, Handle, HandleRegistry, Payload, XcscFfiResultV1};

struct Session {
    runtime: Runtime,
    client: MobileClient,
}
static SESSIONS: OnceLock<HandleRegistry<Arc<Session>>> = OnceLock::new();
fn sessions() -> &'static HandleRegistry<Arc<Session>> {
    SESSIONS.get_or_init(HandleRegistry::default)
}
fn runtime() -> Result<Runtime, FfiError> {
    Runtime::new().map_err(|_| FfiError::internal())
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Pair {
        server: String,
        authorization_code: String,
        name: String,
        installation_id: Uuid,
    },
    Open {
        pairing: Pairing,
    },
    Poll {
        metadata: CaptureMetadata,
        sps: String,
        pps: String,
    },
    Stop,
    Close,
}
fn call_impl(handle: u64, input: &str) -> Result<Payload, FfiError> {
    let operation: Operation =
        serde_json::from_str(input).map_err(|_| FfiError::invalid_argument())?;
    match operation {
        Operation::Pair {
            server,
            authorization_code,
            name,
            installation_id,
        } => {
            if handle != 0 {
                return Err(FfiError::invalid_argument());
            }
            let pairing = runtime()?
                .block_on(mobile::pair(
                    &server,
                    &authorization_code,
                    &name,
                    installation_id,
                ))
                .map_err(|_| {
                    FfiError::internal_with_message(
                        "Unable to pair. Check Server, authorization code and connection.",
                    )
                })?;
            Payload::bytes(serde_json::to_vec(&pairing).map_err(|_| FfiError::internal())?)
        }
        Operation::Open { pairing } => {
            if handle != 0 {
                return Err(FfiError::invalid_argument());
            }
            pairing
                .validate()
                .map_err(|_| FfiError::invalid_argument())?;
            let runtime = runtime()?;
            let client = {
                let _entered = runtime.enter();
                MobileClient::open(pairing).map_err(|_| FfiError::internal())?
            };
            sessions()
                .insert(Arc::new(Session { runtime, client }))
                .map(|handle| Payload::value(handle.to_u64()))
        }
        Operation::Poll { metadata, sps, pps } => {
            if sps.len() > 8192 || pps.len() > 8192 {
                return Err(FfiError::invalid_argument());
            }
            let sps = STANDARD
                .decode(sps)
                .map_err(|_| FfiError::invalid_argument())?;
            let pps = STANDARD
                .decode(pps)
                .map_err(|_| FfiError::invalid_argument())?;
            let session = sessions().get(Handle::from_u64(handle))?;
            let result = session
                .runtime
                .block_on(session.client.poll(&metadata, &sps, &pps))
                .map_err(|_| {
                    FfiError::internal_with_message(
                        "Unable to synchronize camera. Check connection and pairing.",
                    )
                })?;
            Payload::bytes(serde_json::to_vec(&result).map_err(|_| FfiError::internal())?)
        }
        Operation::Stop => {
            sessions().get(Handle::from_u64(handle))?.client.stop();
            Ok(Payload::default())
        }
        Operation::Close => {
            let session = sessions().remove(Handle::from_u64(handle))?;
            session.client.stop();
            Ok(Payload::default())
        }
    }
}
fn frame_impl(handle: u64, frame: &[u8], timestamp_us: u64) -> Result<bool, FfiError> {
    let session = sessions().get(Handle::from_u64(handle))?;
    session
        .client
        .frame(frame, timestamp_us)
        .map_err(|_| FfiError::invalid_argument())
}

/// # Safety
/// `input` must be readable for `input_len` bytes. `output` must be aligned,
/// initialized, exclusively writable, and freed before reuse.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xcoc_call_v1(
    handle: u64,
    input: *const u8,
    input_len: usize,
    output: *mut XcscFfiResultV1,
) -> i32 {
    // SAFETY: The caller supplies the revision 2 buffers described above.
    unsafe {
        ffi::guard(output, || {
            call_impl(handle, ffi::checked_utf8(input, input_len, 65536)?)
        })
    }
}
/// # Safety
/// `frame` must be readable for `frame_len` bytes; the result ownership rules
/// are identical to `xcoc_call_v1`. The frame is copied before return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xcoc_frame_v1(
    handle: u64,
    frame: *const u8,
    frame_len: usize,
    timestamp_us: u64,
    output: *mut XcscFfiResultV1,
) -> i32 {
    // SAFETY: Foundation validates all supplied lengths and output ownership.
    unsafe {
        ffi::guard(output, || {
            frame_impl(
                handle,
                ffi::checked_input(frame, frame_len, MAX_FRAME_BYTES)?,
                timestamp_us,
            )
            .map(|accepted| Payload::value(u64::from(accepted)))
        })
    }
}

#[cfg(any(target_os = "android", feature = "jni-host-tests"))]
mod android;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_input_and_stale_handles_fail_without_network_access() {
        assert!(call_impl(0, r#"{"operation":"open","pairing":{}}"#).is_err());
        assert!(call_impl(123, r#"{"operation":"stop"}"#).is_err());
        assert!(call_impl(0, r#"{"operation":"stop","secret":"private"}"#).is_err());
    }
    #[test]
    fn native_call_uses_owned_foundation_results() {
        let input = br#"{"operation":"stop"}"#;
        let mut result = XcscFfiResultV1::default();
        // SAFETY: All buffers are live, aligned and initialized for this call.
        let status = unsafe { xcoc_call_v1(123, input.as_ptr(), input.len(), &mut result) };
        assert_eq!(status, ffi::XCSC_FFI_INVALID_HANDLE);
        // SAFETY: This result is owned, initialized and freed exactly once.
        unsafe { ffi::xcsc_ffi_result_free_v1(&mut result) };
    }
}
