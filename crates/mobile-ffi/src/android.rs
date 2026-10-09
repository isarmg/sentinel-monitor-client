use super::*;
use ffi::jni::{guard, new_string, read_string};
use jni::{
    EnvUnowned,
    objects::{JByteArray, JClass, JString},
    sys::{jboolean, jlong, jstring},
};

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_sarmg_xcoc_NativeBridge_call(
    mut env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    input: JString,
) -> jstring {
    guard(&mut env, std::ptr::null_mut(), |env| {
        let input = read_string(env, input, 65536)?;
        let payload = call_impl(handle as u64, &input)?;
        // Use the same public C result representation for native JSON commands.
        let document = if payload.bytes.is_empty() {
            serde_json::json!({"value": payload.value}).to_string()
        } else {
            String::from_utf8(payload.bytes).map_err(|_| FfiError::internal())?
        };
        new_string(env, document)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_sarmg_xcoc_NativeBridge_frame(
    mut env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    frame: JByteArray,
    timestamp_us: jlong,
) -> jboolean {
    guard(&mut env, false, |env| {
        let length = frame.len(env).map_err(FfiError::from)?;
        if length == 0 || length > MAX_FRAME_BYTES || timestamp_us < 0 {
            return Err(FfiError::invalid_argument());
        }
        let bytes = env.convert_byte_array(&frame).map_err(FfiError::from)?;
        frame_impl(handle as u64, &bytes, timestamp_us as u64)
    })
}
