use nominal_streaming::prelude::*;
use nominal_streaming::stream::{NominalDatasetStream, NominalDatasetStreamBuilder};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

// ============================================================================
// Error Codes
// ============================================================================

const SUCCESS: c_int = 0;
const ERROR_GENERIC: c_int = -1;
const ERROR_INVALID_HANDLE: c_int = -2;
const ERROR_INVALID_PARAM: c_int = -3;
const ERROR_RUNTIME: c_int = -4;
const ERROR_IO: c_int = -5;
const ERROR_NOT_SUPPORTED: c_int = -6;

// ============================================================================
// Thread-Local Error Storage
// ============================================================================

thread_local! {
    static LAST_ERROR: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
}

fn set_last_error(err: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = Some(err));
}

fn clear_last_error() {
    LAST_ERROR.with(|e| *e.borrow_mut() = None);
}

// ============================================================================
// Global Tokio Runtime
// ============================================================================

static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime")
});

// ============================================================================
// Handle Types and Registries
// ============================================================================

type StreamHandle = u64;
type WriterHandle = u64;

// Store streams
static STREAMS: Lazy<Mutex<HashMap<StreamHandle, Arc<NominalDatasetStream>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// Store writers along with their stream and descriptor to maintain lifetimes
struct WriterState {
    stream: Arc<NominalDatasetStream>,
    descriptor: ChannelDescriptor,
}

static WRITERS: Lazy<Mutex<HashMap<WriterHandle, Arc<Mutex<WriterState>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

static NEXT_STREAM_HANDLE: Lazy<Mutex<u64>> = Lazy::new(|| Mutex::new(1));
static NEXT_WRITER_HANDLE: Lazy<Mutex<u64>> = Lazy::new(|| Mutex::new(1));

fn allocate_stream_handle() -> StreamHandle {
    let mut next = NEXT_STREAM_HANDLE.lock();
    let handle = *next;
    *next += 1;
    handle
}

fn allocate_writer_handle() -> WriterHandle {
    let mut next = NEXT_WRITER_HANDLE.lock();
    let handle = *next;
    *next += 1;
    handle
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Convert C string to Rust String safely
unsafe fn c_str_to_string(c_str: *const c_char) -> Result<String, String> {
    if c_str.is_null() {
        return Err("Null pointer provided".to_string());
    }
    CStr::from_ptr(c_str)
        .to_str()
        .map(|s| s.to_string())
        .map_err(|e| format!("Invalid UTF-8 string: {}", e))
}

/// Parse CSV tags into Vec of tuples
fn parse_tags_csv(tags_csv: &str) -> Vec<(&str, &str)> {
    if tags_csv.is_empty() {
        return Vec::new();
    }
    
    tags_csv
        .split(',')
        .filter_map(|pair| {
            let parts: Vec<&str> = pair.split('=').collect();
            if parts.len() == 2 {
                Some((parts[0].trim(), parts[1].trim()))
            } else {
                None
            }
        })
        .collect()
}

// ============================================================================
// Core FFI Functions
// ============================================================================

/// Initialize a new Nominal stream
#[no_mangle]
pub unsafe extern "C" fn nominal_init(
    token: *const c_char,
    dataset_rid: *const c_char,
    fallback_file_path: *const c_char,
    out_stream_handle: *mut u64,
) -> c_int {
    clear_last_error();

    if out_stream_handle.is_null() {
        set_last_error("Output handle pointer is null".to_string());
        return ERROR_INVALID_PARAM;
    }

    let token_str = if !token.is_null() {
        match c_str_to_string(token) {
            Ok(s) => Some(s),
            Err(e) => {
                set_last_error(format!("Invalid token: {}", e));
                return ERROR_INVALID_PARAM;
            }
        }
    } else {
        None
    };

    let dataset_rid_str = match c_str_to_string(dataset_rid) {
        Ok(s) => s,
        Err(e) => {
            set_last_error(format!("Invalid dataset RID: {}", e));
            return ERROR_INVALID_PARAM;
        }
    };

    let fallback_path_str = if !fallback_file_path.is_null() {
        match c_str_to_string(fallback_file_path) {
            Ok(s) => Some(s),
            Err(e) => {
                set_last_error(format!("Invalid fallback path: {}", e));
                return ERROR_INVALID_PARAM;
            }
        }
    } else {
        None
    };

    let stream = RUNTIME.block_on(async {
        let mut builder = NominalDatasetStreamBuilder::new();

        let should_stream_to_core = token_str.is_some() || std::env::var("NOMINAL_TOKEN").is_ok();

        if should_stream_to_core {
            let token_value = match token_str {
                Some(t) => t,
                None => std::env::var("NOMINAL_TOKEN").unwrap(),
            };

            let bearer_token = match BearerToken::new(&token_value) {
                Ok(t) => t,
                Err(e) => {
                    set_last_error(format!("Invalid bearer token: {}", e));
                    return Err(ERROR_INVALID_PARAM);
                }
            };

            let rid = match ResourceIdentifier::new(&dataset_rid_str) {
                Ok(r) => r,
                Err(e) => {
                    set_last_error(format!("Invalid dataset RID: {}", e));
                    return Err(ERROR_INVALID_PARAM);
                }
            };

            let handle = tokio::runtime::Handle::current();
            builder = builder.stream_to_core(bearer_token, rid, handle);

            if let Some(ref path) = fallback_path_str {
                builder = builder.with_file_fallback(path);
            }
        } else if let Some(ref path) = fallback_path_str {
            builder = builder.stream_to_file(path);
        } else {
            set_last_error("Either token or fallback file path must be provided".to_string());
            return Err(ERROR_INVALID_PARAM);
        }

        Ok(builder.build())
    });

    let stream = match stream {
        Ok(s) => s,
        Err(e) => return e,
    };

    let handle = allocate_stream_handle();
    STREAMS.lock().insert(handle, Arc::new(stream));

    *out_stream_handle = handle;
    SUCCESS
}

/// Create a channel writer
#[no_mangle]
pub unsafe extern "C" fn nominal_create_channel(
    stream_handle: u64,
    channel_name: *const c_char,
    tags_csv: *const c_char,
    out_writer_handle: *mut u64,
) -> c_int {
    clear_last_error();

    if out_writer_handle.is_null() {
        set_last_error("Output handle pointer is null".to_string());
        return ERROR_INVALID_PARAM;
    }

    let stream = {
        let streams = STREAMS.lock();
        match streams.get(&stream_handle) {
            Some(s) => Arc::clone(s),
            None => {
                set_last_error(format!("Invalid stream handle: {}", stream_handle));
                return ERROR_INVALID_HANDLE;
            }
        }
    };

    let channel_name_str = match c_str_to_string(channel_name) {
        Ok(s) => s,
        Err(e) => {
            set_last_error(format!("Invalid channel name: {}", e));
            return ERROR_INVALID_PARAM;
        }
    };

    let tags_csv_str = if !tags_csv.is_null() {
        match c_str_to_string(tags_csv) {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid tags CSV: {}", e));
                return ERROR_INVALID_PARAM;
            }
        }
    } else {
        String::new()
    };

    let tags = parse_tags_csv(&tags_csv_str);

    let descriptor = if tags.is_empty() {
        ChannelDescriptor::new(&channel_name_str)
    } else {
        ChannelDescriptor::with_tags(&channel_name_str, tags)
    };

    let handle = allocate_writer_handle();
    let state = WriterState {
        stream: Arc::clone(&stream),
        descriptor,
    };
    WRITERS.lock().insert(handle, Arc::new(Mutex::new(state)));

    *out_writer_handle = handle;
    SUCCESS
}

/// Push a batch of double data points
#[no_mangle]
pub unsafe extern "C" fn nominal_push_double_batch(
    writer_handle: u64,
    timestamps_ns: *const u64,
    values: *const f64,
    count: usize,
) -> c_int {
    clear_last_error();

    if timestamps_ns.is_null() || values.is_null() {
        set_last_error("Null pointer provided for data arrays".to_string());
        return ERROR_INVALID_PARAM;
    }

    if count == 0 {
        return SUCCESS;
    }

    let writer_arc = {
        let writers = WRITERS.lock();
        match writers.get(&writer_handle) {
            Some(w) => Arc::clone(w),
            None => {
                set_last_error(format!("Invalid writer handle: {}", writer_handle));
                return ERROR_INVALID_HANDLE;
            }
        }
    };

    let timestamps_slice = std::slice::from_raw_parts(timestamps_ns, count);
    let values_slice = std::slice::from_raw_parts(values, count);

    let state_guard = writer_arc.lock();
    let mut writer = state_guard.stream.double_writer(&state_guard.descriptor);
    
    for i in 0..count {
        let timestamp = Duration::from_nanos(timestamps_slice[i]);
        let value = values_slice[i];
        writer.push(timestamp, value);
    }

    SUCCESS
}

/// Close a channel writer and flush remaining data
#[no_mangle]
pub unsafe extern "C" fn nominal_close_channel(writer_handle: u64) -> c_int {
    clear_last_error();

    let writer_arc = {
        let mut writers = WRITERS.lock();
        match writers.remove(&writer_handle) {
            Some(w) => w,
            None => {
                set_last_error(format!("Invalid writer handle: {}", writer_handle));
                return ERROR_INVALID_HANDLE;
            }
        }
    };

    drop(writer_arc);
    SUCCESS
}

/// Shutdown stream and cleanup resources
#[no_mangle]
pub unsafe extern "C" fn nominal_shutdown(stream_handle: u64) -> c_int {
    clear_last_error();

    let _stream = {
        let mut streams = STREAMS.lock();
        match streams.remove(&stream_handle) {
            Some(s) => s,
            None => {
                set_last_error(format!("Invalid stream handle: {}", stream_handle));
                return ERROR_INVALID_HANDLE;
            }
        }
    };

    SUCCESS
}

/// Get the last error message
#[no_mangle]
pub extern "C" fn nominal_get_last_error(
    buffer: *mut c_char,
    buffer_size: usize,
) -> i32 {
    if buffer.is_null() || buffer_size == 0 {
        return ERROR_INVALID_PARAM;
    }

    LAST_ERROR.with(|last_error| {
        let error_msg = last_error.borrow();
        
        let msg = match error_msg.as_ref() {
            Some(s) => s,
            None => {
                unsafe {
                    *buffer = 0;
                }
                return ERROR_GENERIC;
            }
        };

        let error_bytes = msg.as_bytes();
        let copy_len = std::cmp::min(error_bytes.len(), buffer_size - 1);

        unsafe {
            std::ptr::copy_nonoverlapping(
                error_bytes.as_ptr(),
                buffer as *mut u8,
                copy_len,
            );
            *buffer.add(copy_len) = 0;
        }
        
        SUCCESS
    })
}

// ============================================================================
// Lifecycle Control Functions
// ============================================================================

/// Flush all pending data for a stream
#[no_mangle]
pub unsafe extern "C" fn nominal_flush(stream_handle: u64) -> c_int {
    clear_last_error();

    let stream = {
        let streams = STREAMS.lock();
        match streams.get(&stream_handle) {
            Some(s) => Arc::clone(s),
            None => {
                set_last_error(format!("Invalid stream handle: {}", stream_handle));
                return ERROR_INVALID_HANDLE;
            }
        }
    };

    RUNTIME.block_on(async {
        match stream.flush().await {
            Ok(_) => SUCCESS,
            Err(e) => {
                set_last_error(format!("Flush failed: {}", e));
                ERROR_RUNTIME
            }
        }
    })
}

/// Flush a specific channel writer
#[no_mangle]
pub unsafe extern "C" fn nominal_flush_channel(writer_handle: u64) -> c_int {
    clear_last_error();

    let writer_arc = {
        let writers = WRITERS.lock();
        match writers.get(&writer_handle) {
            Some(w) => Arc::clone(w),
            None => {
                set_last_error(format!("Invalid writer handle: {}", writer_handle));
                return ERROR_INVALID_HANDLE;
            }
        }
    };

    let state_guard = writer_arc.lock();
    
    RUNTIME.block_on(async {
        match state_guard.stream.flush().await {
            Ok(_) => SUCCESS,
            Err(e) => {
                set_last_error(format!("Channel flush failed: {}", e));
                ERROR_RUNTIME
            }
        }
    })
}

// ============================================================================
// Diagnostics & Monitoring Functions
// ============================================================================

/// Get the number of active stream handles
#[no_mangle]
pub extern "C" fn nominal_get_active_streams() -> c_int {
    STREAMS.lock().len() as c_int
}

/// Get the number of active writer handles
#[no_mangle]
pub extern "C" fn nominal_get_active_writers() -> c_int {
    WRITERS.lock().len() as c_int
}

/// Check if a stream handle is valid
#[no_mangle]
pub extern "C" fn nominal_is_stream_valid(stream_handle: u64) -> c_int {
    let streams = STREAMS.lock();
    if streams.contains_key(&stream_handle) {
        1
    } else {
        0
    }
}

/// Check if a writer handle is valid
#[no_mangle]
pub extern "C" fn nominal_is_writer_valid(writer_handle: u64) -> c_int {
    let writers = WRITERS.lock();
    if writers.contains_key(&writer_handle) {
        1
    } else {
        0
    }
}

// ============================================================================
// Bulk Operations Functions
// ============================================================================

/// Push batch of int64 data points
#[no_mangle]
pub unsafe extern "C" fn nominal_push_int64_batch(
    writer_handle: u64,
    timestamps_ns: *const u64,
    values: *const i64,
    count: usize,
) -> c_int {
    clear_last_error();

    if timestamps_ns.is_null() || values.is_null() {
        set_last_error("Null pointer provided for data arrays".to_string());
        return ERROR_INVALID_PARAM;
    }

    if count == 0 {
        return SUCCESS;
    }

    let writer_arc = {
        let writers = WRITERS.lock();
        match writers.get(&writer_handle) {
            Some(w) => Arc::clone(w),
            None => {
                set_last_error(format!("Invalid writer handle: {}", writer_handle));
                return ERROR_INVALID_HANDLE;
            }
        }
    };

    let timestamps_slice = std::slice::from_raw_parts(timestamps_ns, count);
    let values_slice = std::slice::from_raw_parts(values, count);

    let state_guard = writer_arc.lock();
    let mut writer = state_guard.stream.int64_writer(&state_guard.descriptor);
    
    for i in 0..count {
        let timestamp = Duration::from_nanos(timestamps_slice[i]);
        let value = values_slice[i];
        writer.push(timestamp, value);
    }

    SUCCESS
}

/// Push batch of boolean data points
#[no_mangle]
pub unsafe extern "C" fn nominal_push_bool_batch(
    writer_handle: u64,
    timestamps_ns: *const u64,
    values: *const u8,  // 0 = false, non-zero = true
    count: usize,
) -> c_int {
    clear_last_error();

    if timestamps_ns.is_null() || values.is_null() {
        set_last_error("Null pointer provided for data arrays".to_string());
        return ERROR_INVALID_PARAM;
    }

    if count == 0 {
        return SUCCESS;
    }

    let writer_arc = {
        let writers = WRITERS.lock();
        match writers.get(&writer_handle) {
            Some(w) => Arc::clone(w),
            None => {
                set_last_error(format!("Invalid writer handle: {}", writer_handle));
                return ERROR_INVALID_HANDLE;
            }
        }
    };

    let timestamps_slice = std::slice::from_raw_parts(timestamps_ns, count);
    let values_slice = std::slice::from_raw_parts(values, count);

    let state_guard = writer_arc.lock();
    let mut writer = state_guard.stream.bool_writer(&state_guard.descriptor);
    
    for i in 0..count {
        let timestamp = Duration::from_nanos(timestamps_slice[i]);
        let value = values_slice[i] != 0;
        writer.push(timestamp, value);
    }

    SUCCESS
}

/// Push batch of string data points
#[no_mangle]
pub unsafe extern "C" fn nominal_push_string_batch(
    writer_handle: u64,
    timestamps_ns: *const u64,
    values: *const *const c_char,  // Array of C string pointers
    count: usize,
) -> c_int {
    clear_last_error();

    if timestamps_ns.is_null() || values.is_null() {
        set_last_error("Null pointer provided for data arrays".to_string());
        return ERROR_INVALID_PARAM;
    }

    if count == 0 {
        return SUCCESS;
    }

    let writer_arc = {
        let writers = WRITERS.lock();
        match writers.get(&writer_handle) {
            Some(w) => Arc::clone(w),
            None => {
                set_last_error(format!("Invalid writer handle: {}", writer_handle));
                return ERROR_INVALID_HANDLE;
            }
        }
    };

    let timestamps_slice = std::slice::from_raw_parts(timestamps_ns, count);
    let values_slice = std::slice::from_raw_parts(values, count);

    let state_guard = writer_arc.lock();
    let mut writer = state_guard.stream.string_writer(&state_guard.descriptor);
    
    for i in 0..count {
        let timestamp = Duration::from_nanos(timestamps_slice[i]);
        
        // Convert C string to Rust string
        let value_str = match c_str_to_string(values_slice[i]) {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid string at index {}: {}", i, e));
                return ERROR_INVALID_PARAM;
            }
        };
        
        writer.push(timestamp, &value_str);
    }

    SUCCESS
}

// ============================================================================
// Stream Information Functions
// ============================================================================

/// Get the channel name for a writer
#[no_mangle]
pub unsafe extern "C" fn nominal_get_channel_name(
    writer_handle: u64,
    buffer: *mut c_char,
    buffer_size: usize,
) -> c_int {
    clear_last_error();

    if buffer.is_null() || buffer_size == 0 {
        set_last_error("Invalid buffer parameters".to_string());
        return ERROR_INVALID_PARAM;
    }

    let writer_arc = {
        let writers = WRITERS.lock();
        match writers.get(&writer_handle) {
            Some(w) => Arc::clone(w),
            None => {
                set_last_error(format!("Invalid writer handle: {}", writer_handle));
                return ERROR_INVALID_HANDLE;
            }
        }
    };

    let state_guard = writer_arc.lock();
    let channel_name = state_guard.descriptor.name();
    
    let name_bytes = channel_name.as_bytes();
    let copy_len = std::cmp::min(name_bytes.len(), buffer_size - 1);

    std::ptr::copy_nonoverlapping(
        name_bytes.as_ptr(),
        buffer as *mut u8,
        copy_len,
    );
    *buffer.add(copy_len) = 0;

    SUCCESS
}

/// Get library version information
#[no_mangle]
pub unsafe extern "C" fn nominal_get_version(
    buffer: *mut c_char,
    buffer_size: usize,
) -> c_int {
    if buffer.is_null() || buffer_size == 0 {
        return ERROR_INVALID_PARAM;
    }

    let version = env!("CARGO_PKG_VERSION");
    let version_bytes = version.as_bytes();
    let copy_len = std::cmp::min(version_bytes.len(), buffer_size - 1);

    std::ptr::copy_nonoverlapping(
        version_bytes.as_ptr(),
        buffer as *mut u8,
        copy_len,
    );
    *buffer.add(copy_len) = 0;

    SUCCESS
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tags_csv() {
        let tags = parse_tags_csv("key1=value1,key2=value2");
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0], ("key1", "value1"));
        assert_eq!(tags[1], ("key2", "value2"));
    }

    #[test]
    fn test_parse_tags_csv_empty() {
        let tags = parse_tags_csv("");
        assert_eq!(tags.len(), 0);
    }

    #[test]
    fn test_handle_allocation() {
        let h1 = allocate_stream_handle();
        let h2 = allocate_stream_handle();
        assert_ne!(h1, h2);
        assert!(h2 > h1);
    }
}