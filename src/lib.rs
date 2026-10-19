mod proxy;

use nominal_streaming::client::conjure::http::client::ConjureRuntime;
use nominal_streaming::client::conjure::runtime::config::{
    BasicCredentials, HostAndPort, HttpProxyConfig, ProxyConfig,
};
use nominal_streaming::client::conjure::runtime::{Agent, Client, Idempotency, UserAgent};
use nominal_streaming::client::NominalApiClients;
use nominal_streaming::consumer::{
    AvroFileConsumer, ListeningWriteRequestConsumer, NominalCoreConsumer,
    RequestConsumerWithFallback,
};
use nominal_streaming::listener::{LoggingListener, NominalStreamListener};
use nominal_streaming::prelude::*;
use nominal_streaming::stream::{
    NominalDatasetStream, NominalDatasetStreamBuilder, NominalStreamOpts,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use proxy::{is_direct, ProxyServer, ProxySettings};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use url::Url;

// ============================================================================
// Error Codes
// ============================================================================

const SUCCESS: c_int = 0;
const ERROR_INVALID_HANDLE: c_int = -2;
const ERROR_INVALID_PARAM: c_int = -3;
const ERROR_RUNTIME: c_int = -4;
const ERROR_IO: c_int = -5;

// ============================================================================
// Thread-Local Error Storage
// ============================================================================

thread_local! {
    static LAST_ERROR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
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
// The writer has a lifetime tied to the stream and descriptor
struct WriterState {
    stream: Arc<NominalDatasetStream>,
    descriptor: ChannelDescriptor,
    // We can't store the writer directly due to lifetime constraints
    // So we'll recreate it on each push operation
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

/// The proxy to reach the Nominal API through, or `None` to connect directly
fn api_proxy(proxy: Option<&str>) -> Result<Option<ProxyServer>, String> {
    let settings = ProxySettings::detect(proxy)?;
    let api = Url::parse(&NominalStreamOpts::default().base_api_url)
        .map_err(|e| format!("Invalid API URL: {}", e))?;
    Ok(settings
        .for_destination(api.scheme(), api.host_str().unwrap_or(""))
        .cloned())
}

/// Convert a proxy into conjure-runtime's form
fn conjure_proxy(server: &ProxyServer) -> ProxyConfig {
    ProxyConfig::Http(
        HttpProxyConfig::builder()
            .host_and_port(HostAndPort::new(&server.host, server.port))
            .credentials(
                server
                    .credentials
                    .as_ref()
                    .map(|(user, password)| BasicCredentials::new(user, password)),
            )
            .build(),
    )
}

/// Build the Nominal API clients with a proxy
///
/// Uses the settings of nominal-streaming's `async_conjure_streaming_client`
/// and `async_conjure_client`, which cannot take a proxy.
fn api_clients(base_url: &Url, proxy: ProxyConfig) -> Result<NominalApiClients, String> {
    let user_agent = || {
        UserAgent::new(Agent::new(
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
        ))
    };

    let streaming = Client::builder()
        .service("core-streaming-rs")
        .user_agent(user_agent())
        .uri(base_url.clone())
        .proxy(proxy.clone())
        .connect_timeout(Duration::from_secs(1))
        .read_timeout(Duration::from_secs(2))
        .write_timeout(Duration::from_secs(2))
        .backoff_slot_size(Duration::from_millis(10))
        .max_num_retries(2)
        // enables retries for POST endpoints like the streaming ingest one
        .idempotency(Idempotency::Always)
        .build()
        .map_err(|e| format!("Failed to create streaming client: {}", e.cause()))?;

    let services = Client::builder()
        .service("upload-ingest")
        .user_agent(user_agent())
        .uri(base_url.clone())
        .proxy(proxy)
        .build()
        .map_err(|e| format!("Failed to create upload/ingest client: {}", e.cause()))?;

    Ok(NominalApiClients::from_conjure_clients(
        streaming,
        services,
        &Arc::new(ConjureRuntime::default()),
    ))
}

/// Build a stream to core whose connection to Nominal goes through `server`
///
/// Assembled like nominal-streaming's NominalDatasetStreamBuilder::build, whose
/// clients can't take a proxy.
fn stream_through_proxy(
    server: &ProxyServer,
    bearer_token: BearerToken,
    rid: ResourceIdentifier,
    handle: tokio::runtime::Handle,
    fallback_path: Option<&str>,
) -> Result<NominalDatasetStream, c_int> {
    let opts = NominalStreamOpts::default();
    let clients = match Url::parse(&opts.base_api_url)
        .map_err(|e| format!("Invalid API URL: {}", e))
        .and_then(|url| api_clients(&url, conjure_proxy(server)))
    {
        Ok(c) => c,
        Err(e) => {
            set_last_error(e);
            return Err(ERROR_RUNTIME);
        }
    };

    let core = NominalCoreConsumer::new(clients, handle, bearer_token, rid.clone());
    let listeners: Vec<Arc<dyn NominalStreamListener>> = vec![Arc::new(LoggingListener)];

    match fallback_path {
        Some(path) => {
            let fallback = match AvroFileConsumer::new_with_full_path(path, true, Some(rid)) {
                Ok(f) => f,
                Err(e) => {
                    set_last_error(format!("Failed to open fallback file: {}", e));
                    return Err(ERROR_IO);
                }
            };
            let consumer = RequestConsumerWithFallback::new(core, fallback);
            Ok(NominalDatasetStream::new_with_consumer(
                ListeningWriteRequestConsumer::new(consumer, listeners),
                opts,
            ))
        }
        None => Ok(NominalDatasetStream::new_with_consumer(
            ListeningWriteRequestConsumer::new(core, listeners),
            opts,
        )),
    }
}

/// The message of a caught panic
fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    if let Some(s) = panic.downcast_ref::<&str>() {
        s
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s
    } else {
        "unknown error"
    }
}

// ============================================================================
// FFI Functions
// ============================================================================

/// Initialize a new Nominal stream
/// 
/// # Arguments
/// * `token` - Nominal API token (can be null to use env var NOMINAL_TOKEN)
/// * `dataset_rid` - Dataset RID (e.g., "ri.catalog.main.dataset....")
/// * `fallback_file_path` - Path for fallback AVRO file (can be null for no fallback)
/// * `out_stream_handle` - Output pointer for stream handle
///
/// Connects directly, without a proxy; `nominal_init_with_proxy` adds one.
///
/// # Returns
/// 0 on success, negative error code on failure
///
/// # Safety
/// Each string must be null or a valid null-terminated string, and
/// `out_stream_handle` must be null or valid for writes.
#[no_mangle]
pub unsafe extern "C" fn nominal_init(
    token: *const c_char,
    dataset_rid: *const c_char,
    fallback_file_path: *const c_char,
    out_stream_handle: *mut u64,
) -> c_int {
    clear_last_error();
    init_stream(token, dataset_rid, fallback_file_path, None, out_stream_handle)
}

/// Initialize a new Nominal stream, choosing the proxy used to reach Nominal
///
/// # Arguments
/// * `token` - Nominal API token (can be null to use env var NOMINAL_TOKEN)
/// * `dataset_rid` - Dataset RID (e.g., "ri.catalog.main.dataset....")
/// * `fallback_file_path` - Path for fallback AVRO file (can be null for no fallback)
/// * `proxy` - `http://host:port`, optionally with `user:password@` for Basic
///   authentication, or `direct` for no proxy. Null or empty detects it from
///   HTTPS_PROXY / ALL_PROXY (honoring NO_PROXY), then the Windows proxy setting
///   (honoring its bypass list). PAC scripts and NTLM authentication are not supported.
///   With `direct`, or when no proxy is found, the stream is created exactly as
///   `nominal_init` creates it.
/// * `out_stream_handle` - Output pointer for stream handle
///
/// # Returns
/// 0 on success, negative error code on failure
///
/// # Safety
/// Each string must be null or a valid null-terminated string, and
/// `out_stream_handle` must be null or valid for writes.
#[no_mangle]
pub unsafe extern "C" fn nominal_init_with_proxy(
    token: *const c_char,
    dataset_rid: *const c_char,
    fallback_file_path: *const c_char,
    proxy: *const c_char,
    out_stream_handle: *mut u64,
) -> c_int {
    clear_last_error();

    // Parse proxy
    let proxy_str = if !proxy.is_null() {
        match c_str_to_string(proxy) {
            Ok(s) => Some(s),
            Err(e) => {
                set_last_error(format!("Invalid proxy: {}", e));
                return ERROR_INVALID_PARAM;
            }
        }
    } else {
        None
    };

    // `direct` skips proxy support entirely
    let proxy_server = if proxy_str.as_deref().is_some_and(is_direct) {
        None
    } else {
        match api_proxy(proxy_str.as_deref()) {
            Ok(p) => p,
            Err(e) => {
                set_last_error(e);
                return ERROR_INVALID_PARAM;
            }
        }
    };

    init_stream(
        token,
        dataset_rid,
        fallback_file_path,
        proxy_server.as_ref(),
        out_stream_handle,
    )
}

/// Create a stream for `nominal_init` and `nominal_init_with_proxy`.
/// Without a proxy, nominal-streaming builds the stream itself.
unsafe fn init_stream(
    token: *const c_char,
    dataset_rid: *const c_char,
    fallback_file_path: *const c_char,
    proxy: Option<&ProxyServer>,
    out_stream_handle: *mut u64,
) -> c_int {
    // Validate output pointer
    if out_stream_handle.is_null() {
        set_last_error("Output handle pointer is null".to_string());
        return ERROR_INVALID_PARAM;
    }

    // Parse token
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

    // Parse dataset RID
    let dataset_rid_str = match c_str_to_string(dataset_rid) {
        Ok(s) => s,
        Err(e) => {
            set_last_error(format!("Invalid dataset RID: {}", e));
            return ERROR_INVALID_PARAM;
        }
    };

    // Parse fallback path
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

    // Build the stream
    let stream = RUNTIME.block_on(async {
        let mut builder = NominalDatasetStreamBuilder::new();

        // Determine if we should stream to core
        let should_stream_to_core = token_str.is_some() || std::env::var("NOMINAL_TOKEN").is_ok();

        if should_stream_to_core {
            // Get token (from param or env)
            let token_value = match token_str {
                Some(t) => t,
                None => std::env::var("NOMINAL_TOKEN").unwrap(),
            };

            // Create BearerToken
            let bearer_token = match BearerToken::new(&token_value) {
                Ok(t) => t,
                Err(e) => {
                    set_last_error(format!("Invalid bearer token: {}", e));
                    return Err(ERROR_INVALID_PARAM);
                }
            };

            // Create ResourceIdentifier
            let rid = match ResourceIdentifier::new(&dataset_rid_str) {
                Ok(r) => r,
                Err(e) => {
                    set_last_error(format!("Invalid dataset RID: {}", e));
                    return Err(ERROR_INVALID_PARAM);
                }
            };

            // Get current runtime handle
            let handle = tokio::runtime::Handle::current();

            if let Some(server) = proxy {
                return stream_through_proxy(
                    server,
                    bearer_token,
                    rid,
                    handle,
                    fallback_path_str.as_deref(),
                );
            }

            // Stream to core
            builder = builder.stream_to_core(bearer_token, rid, handle);

            // Add file fallback if provided
            if let Some(ref path) = fallback_path_str {
                builder = builder.with_file_fallback(path);
            }
        } else if let Some(ref path) = fallback_path_str {
            // No token, just stream to file
            builder = builder.stream_to_file(path);
        } else {
            set_last_error("Either token or fallback file path must be provided".to_string());
            return Err(ERROR_INVALID_PARAM);
        }

        // nominal-streaming panics when it can't open the stream's file; catch that
        // rather than let it unwind into LabVIEW
        catch_unwind(AssertUnwindSafe(|| builder.build())).map_err(|panic| {
            set_last_error(format!(
                "Failed to open the stream's file: {}",
                panic_message(&*panic)
            ));
            ERROR_IO
        })
    });

    let stream = match stream {
        Ok(s) => s,
        Err(e) => return e,
    };

    // Allocate handle and store stream
    let handle = allocate_stream_handle();
    STREAMS.lock().insert(handle, Arc::new(stream));

    *out_stream_handle = handle;
    SUCCESS
}

/// Create a channel writer
/// 
/// # Arguments
/// * `stream_handle` - Stream handle from nominal_init
/// * `channel_name` - Name of the channel
/// * `tags_csv` - Comma-separated key=value pairs (e.g., "exp=123,sensor=front")
/// * `out_writer_handle` - Output pointer for writer handle
///
/// # Returns
/// 0 on success, negative error code on failure
///
/// # Safety
/// `channel_name` and `tags_csv` must each be null or a valid null-terminated
/// string, and `out_writer_handle` must be null or valid for writes.
#[no_mangle]
pub unsafe extern "C" fn nominal_create_channel(
    stream_handle: u64,
    channel_name: *const c_char,
    tags_csv: *const c_char,
    out_writer_handle: *mut u64,
) -> c_int {
    clear_last_error();

    // Validate output pointer
    if out_writer_handle.is_null() {
        set_last_error("Output handle pointer is null".to_string());
        return ERROR_INVALID_PARAM;
    }

    // Get stream
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

    // Parse channel name
    let channel_name_str = match c_str_to_string(channel_name) {
        Ok(s) => s,
        Err(e) => {
            set_last_error(format!("Invalid channel name: {}", e));
            return ERROR_INVALID_PARAM;
        }
    };

    // Parse tags
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

    // Create channel descriptor
    let descriptor = if tags.is_empty() {
        ChannelDescriptor::new(&channel_name_str)
    } else {
        ChannelDescriptor::with_tags(&channel_name_str, tags)
    };

    // Allocate handle and store writer state
    // We store the stream and descriptor, and create the writer on-demand
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
/// 
/// # Arguments
/// * `writer_handle` - Writer handle from nominal_create_channel
/// * `timestamps_ns` - Array of timestamps in nanoseconds since Unix epoch
/// * `values` - Array of double values
/// * `count` - Number of points in the arrays
///
/// # Returns
/// 0 on success, negative error code on failure
///
/// # Safety
/// `timestamps_ns` and `values` must each be null or valid for reads of
/// `count` elements.
#[no_mangle]
pub unsafe extern "C" fn nominal_push_double_batch(
    writer_handle: u64,
    timestamps_ns: *const u64,
    values: *const f64,
    count: usize,
) -> c_int {
    clear_last_error();

    // Validate pointers
    if timestamps_ns.is_null() || values.is_null() {
        set_last_error("Null pointer provided for data arrays".to_string());
        return ERROR_INVALID_PARAM;
    }

    if count == 0 {
        return SUCCESS; // Nothing to do
    }

    // Get writer state
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

    // Convert arrays to slices
    let timestamps_slice = std::slice::from_raw_parts(timestamps_ns, count);
    let values_slice = std::slice::from_raw_parts(values, count);

    // Get the writer state and push each point
    let state_guard = writer_arc.lock();
    let mut writer = state_guard
        .stream
        .double_writer(state_guard.descriptor.clone());
    
    for i in 0..count {
        let timestamp = Duration::from_nanos(timestamps_slice[i]);
        let value = values_slice[i];
        writer.push(timestamp, value);
    }

    SUCCESS
}

/// Close a channel writer and flush remaining data
/// 
/// # Arguments
/// * `writer_handle` - Writer handle from nominal_create_channel
/// 
/// # Returns
/// 0 on success, negative error code on failure
#[no_mangle]
pub extern "C" fn nominal_close_channel(writer_handle: u64) -> c_int {
    clear_last_error();

    // Remove writer from registry
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

    // Drop the writer - this should trigger any cleanup
    drop(writer_arc);

    SUCCESS
}

/// Shutdown stream and cleanup resources
/// 
/// # Arguments
/// * `stream_handle` - Stream handle from nominal_init
/// 
/// # Returns
/// 0 on success, negative error code on failure
#[no_mangle]
pub extern "C" fn nominal_shutdown(stream_handle: u64) -> c_int {
    clear_last_error();

    // Remove stream from registry
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

    // Stream will be dropped here, triggering cleanup
    SUCCESS
}

/// Get the last error message
/// 
/// # Arguments
/// * `buffer` - Output buffer for error message
/// * `buffer_size` - Size of the buffer
/// 
/// # Returns
/// 0 on success, negative error code if no error or buffer too small
///
/// # Safety
/// `buffer` must be null or valid for writes of `buffer_size` bytes.
#[no_mangle]
pub unsafe extern "C" fn nominal_get_last_error(
    buffer: *mut c_char,
    buffer_size: usize,
) -> i32 {
    if buffer.is_null() || buffer_size == 0 {
        return -3;
    }

    LAST_ERROR.with(|last_error| {
        let error_msg = last_error.borrow();
        
        // Handle Option<String>
        let msg = match error_msg.as_ref() {
            Some(s) => s,
            None => {
                // No error stored, write empty string
                unsafe {
                    *buffer = 0;
                }
                return -1;
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
            // Null terminate
            *buffer.add(copy_len) = 0;
        }
        
        0
    })
}


// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

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

    const TEST_RID: &str = "ri.catalog.main.dataset.00000000-0000-0000-0000-000000000000";

    /// Listens like a proxy, refuses every connection, and sends back each request head
    fn refusing_proxy() -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && matches!(connection.read(&mut byte), Ok(1)) {
                    head.push(byte[0]);
                }
                let _ =
                    connection.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
                if tx
                    .send(String::from_utf8_lossy(&head).into_owned())
                    .is_err()
                {
                    return;
                }
            }
        });
        (address, rx)
    }

    fn init_with_proxy(fallback: Option<&str>, proxy: &str) -> (c_int, u64) {
        let token = CString::new("test-token").unwrap();
        let rid = CString::new(TEST_RID).unwrap();
        let fallback = fallback.map(|f| CString::new(f).unwrap());
        let proxy = CString::new(proxy).unwrap();
        let mut stream = 0;
        let result = unsafe {
            nominal_init_with_proxy(
                token.as_ptr(),
                rid.as_ptr(),
                fallback.as_ref().map_or(std::ptr::null(), |f| f.as_ptr()),
                proxy.as_ptr(),
                &mut stream,
            )
        };
        (result, stream)
    }

    /// Streams one point and returns the first request the proxy receives
    fn first_request_through(proxy: &str, rx: &std::sync::mpsc::Receiver<String>) -> String {
        let (result, stream) = init_with_proxy(None, proxy);
        assert_eq!(result, SUCCESS, "{}", last_error());

        let channel = CString::new("proxy_test").unwrap();
        let mut writer = 0;
        let timestamps = [1_700_000_000_000_000_000u64];
        let values = [1.0f64];
        unsafe {
            assert_eq!(
                nominal_create_channel(stream, channel.as_ptr(), std::ptr::null(), &mut writer),
                SUCCESS
            );
            assert_eq!(
                nominal_push_double_batch(writer, timestamps.as_ptr(), values.as_ptr(), 1),
                SUCCESS
            );
        }

        let head = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("the stream never connected to the proxy");

        assert_eq!(nominal_close_channel(writer), SUCCESS);
        assert_eq!(nominal_shutdown(stream), SUCCESS);
        head
    }

    fn last_error() -> String {
        let mut buffer = [0 as c_char; 512];
        unsafe { nominal_get_last_error(buffer.as_mut_ptr(), buffer.len()) };
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn test_stream_connects_through_proxy() {
        let (address, rx) = refusing_proxy();
        let head = first_request_through(&format!("http://{}", address), &rx);
        assert!(
            head.starts_with("CONNECT api.gov.nominal.io:443 HTTP/1.1\r\n"),
            "{}",
            head
        );
        assert!(
            !head.to_ascii_lowercase().contains("proxy-authorization"),
            "{}",
            head
        );
    }

    #[test]
    fn test_stream_sends_proxy_credentials() {
        let (address, rx) = refusing_proxy();
        let head = first_request_through(&format!("http://user:pass@{}", address), &rx);
        assert!(
            head.starts_with("CONNECT api.gov.nominal.io:443 HTTP/1.1\r\n"),
            "{}",
            head
        );
        // base64 of "user:pass"
        assert!(
            head.to_ascii_lowercase()
                .contains("proxy-authorization: basic dxnlcjpwyxnz\r\n"),
            "{}",
            head
        );
    }

    #[test]
    fn test_init_rejects_unsupported_proxy() {
        let (result, _) = init_with_proxy(None, "socks5://127.0.0.1:1080");
        assert_eq!(result, ERROR_INVALID_PARAM);
        assert!(last_error().contains("only http://"), "{}", last_error());
    }

    #[test]
    fn test_direct_reports_unopenable_fallback_file() {
        // A directory cannot be opened as the fallback file
        let directory = std::env::temp_dir();
        let (result, _) = init_with_proxy(Some(directory.to_str().unwrap()), "direct");
        assert_eq!(result, ERROR_IO);
        assert!(last_error().contains("Failed to open"), "{}", last_error());
    }

    #[test]
    fn test_proxy_reports_unopenable_fallback_file() {
        // Nothing listens on the discard port; the file fails before any connection
        let directory = std::env::temp_dir();
        let (result, _) = init_with_proxy(Some(directory.to_str().unwrap()), "127.0.0.1:9");
        assert_eq!(result, ERROR_IO);
        assert!(last_error().contains("fallback file"), "{}", last_error());
    }

    #[test]
    fn test_direct_stream_starts() {
        let (result, stream) = init_with_proxy(None, "DIRECT");
        assert_eq!(result, SUCCESS, "{}", last_error());
        assert_eq!(nominal_shutdown(stream), SUCCESS);
    }
}