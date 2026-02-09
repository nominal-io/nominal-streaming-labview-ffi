/**
 * @file nominal_labview_ffi.h
 * @brief C FFI header for Nominal Streaming LabVIEW integration
 * 
 * This header provides the C interface for the Nominal Streaming library,
 * designed for use with LabVIEW Call Library Function Nodes.
 * 
 * Platform-specific library names:
 *   Windows: nominal_labview_ffi.dll
 *   Linux:   libnominal_labview_ffi.so
 *   macOS:   libnominal_labview_ffi.dylib
 */

#ifndef NOMINAL_LABVIEW_FFI_H
#define NOMINAL_LABVIEW_FFI_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ============================================================================
 * Type Definitions
 * ========================================================================= */

/**
 * Handle to a Nominal dataset stream
 * Obtained from nominal_init(), released with nominal_shutdown()
 */
typedef uint64_t NominalStreamHandle;

/**
 * Handle to a channel writer
 * Obtained from nominal_create_channel(), released with nominal_close_channel()
 */
typedef uint64_t NominalWriterHandle;

/* ============================================================================
 * Error Codes
 * ========================================================================= */

#define NOMINAL_SUCCESS             0   /**< Operation successful */
#define NOMINAL_ERROR_GENERIC      -1   /**< Generic error */
#define NOMINAL_ERROR_INVALID_HANDLE -2 /**< Invalid stream or writer handle */
#define NOMINAL_ERROR_INVALID_PARAM  -3 /**< Invalid parameter (null pointer, etc.) */
#define NOMINAL_ERROR_RUNTIME       -4   /**< Runtime error (Tokio/network issue) */
#define NOMINAL_ERROR_IO            -5   /**< File I/O error */
#define NOMINAL_ERROR_NOT_SUPPORTED -6   /**< Operation not supported */

/* ============================================================================
 * Core Lifecycle Functions
 * ========================================================================= */

/**
 * Initialize a new Nominal dataset stream
 * 
 * @param token API authentication token (can be NULL to use NOMINAL_TOKEN env var)
 * @param dataset_rid Dataset resource identifier (e.g., "ri.catalog.main.dataset....")
 * @param fallback_file_path Path for fallback AVRO file (can be NULL for no fallback)
 * @param out_stream_handle [OUT] Pointer to receive the stream handle
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * @note Either token or NOMINAL_TOKEN environment variable must be set,
 *       OR fallback_file_path must be provided for file-only mode
 * 
 * Example (LabVIEW):
 *   - token: String (pass by pointer) or NULL
 *   - dataset_rid: String (pass by pointer)
 *   - fallback_file_path: String (pass by pointer) or NULL
 *   - out_stream_handle: U64 (pass by pointer)
 *   - Return: I32 (by value)
 */
int32_t nominal_init(
    const char* token,
    const char* dataset_rid,
    const char* fallback_file_path,
    uint64_t* out_stream_handle
);

/**
 * Create a channel writer for a specific data channel
 * 
 * @param stream_handle Stream handle from nominal_init()
 * @param channel_name Name of the channel (e.g., "temperature", "pressure")
 * @param tags_csv Comma-separated key=value pairs (e.g., "experiment=123,sensor=front")
 *                 Can be NULL or empty string for no tags
 * @param out_writer_handle [OUT] Pointer to receive the writer handle
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * Example (LabVIEW):
 *   - stream_handle: U64 (pass by value)
 *   - channel_name: String (pass by pointer)
 *   - tags_csv: String (pass by pointer) or NULL
 *   - out_writer_handle: U64 (pass by pointer)
 *   - Return: I32 (by value)
 */
int32_t nominal_create_channel(
    uint64_t stream_handle,
    const char* channel_name,
    const char* tags_csv,
    uint64_t* out_writer_handle
);

/**
 * Close a channel writer and flush remaining data
 * 
 * @param writer_handle Writer handle from nominal_create_channel()
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * Example (LabVIEW):
 *   - writer_handle: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_close_channel(uint64_t writer_handle);

/**
 * Shutdown stream and cleanup all resources
 * 
 * @param stream_handle Stream handle from nominal_init()
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * @note This will flush all pending data before shutting down
 * 
 * Example (LabVIEW):
 *   - stream_handle: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_shutdown(uint64_t stream_handle);

/**
 * Get the last error message
 * 
 * @param buffer [OUT] Buffer to receive error message string
 * @param buffer_size Size of the buffer in bytes
 * 
 * @return NOMINAL_SUCCESS if error retrieved, negative code if no error or buffer too small
 * 
 * Example (LabVIEW):
 *   - buffer: String (pass by pointer, pre-allocated with buffer_size bytes)
 *   - buffer_size: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_get_last_error(char* buffer, size_t buffer_size);

/* ============================================================================
 * Data Push Functions - Basic Types
 * ========================================================================= */

/**
 * Push a batch of double-precision floating point data
 * 
 * @param writer_handle Writer handle from nominal_create_channel()
 * @param timestamps_ns Array of timestamps in nanoseconds since Unix epoch
 * @param values Array of double values
 * @param count Number of points in both arrays (must be same length)
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * @note This is the hot path - called frequently. Arrays must have same length.
 * 
 * Example (LabVIEW):
 *   - writer_handle: U64 (pass by value)
 *   - timestamps_ns: Array of U64 (pass by pointer)
 *   - values: Array of DBL (pass by pointer)
 *   - count: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_push_double_batch(
    uint64_t writer_handle,
    const uint64_t* timestamps_ns,
    const double* values,
    size_t count
);

/**
 * Push a batch of 64-bit integer data
 * 
 * @param writer_handle Writer handle from nominal_create_channel()
 * @param timestamps_ns Array of timestamps in nanoseconds since Unix epoch
 * @param values Array of int64 values
 * @param count Number of points in both arrays
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * Example (LabVIEW):
 *   - writer_handle: U64 (pass by value)
 *   - timestamps_ns: Array of U64 (pass by pointer)
 *   - values: Array of I64 (pass by pointer)
 *   - count: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_push_int64_batch(
    uint64_t writer_handle,
    const uint64_t* timestamps_ns,
    const int64_t* values,
    size_t count
);

/**
 * Push a batch of boolean data
 * 
 * @param writer_handle Writer handle from nominal_create_channel()
 * @param timestamps_ns Array of timestamps in nanoseconds since Unix epoch
 * @param values Array of boolean values (0 = false, non-zero = true)
 * @param count Number of points in both arrays
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * Example (LabVIEW):
 *   - writer_handle: U64 (pass by value)
 *   - timestamps_ns: Array of U64 (pass by pointer)
 *   - values: Array of U8 (pass by pointer)
 *   - count: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_push_bool_batch(
    uint64_t writer_handle,
    const uint64_t* timestamps_ns,
    const uint8_t* values,
    size_t count
);

/**
 * Push a batch of string data
 * 
 * @param writer_handle Writer handle from nominal_create_channel()
 * @param timestamps_ns Array of timestamps in nanoseconds since Unix epoch
 * @param values Array of C string pointers (null-terminated strings)
 * @param count Number of points in both arrays
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * @note Each string in the values array must be null-terminated
 * 
 * Example (LabVIEW):
 *   - writer_handle: U64 (pass by value)
 *   - timestamps_ns: Array of U64 (pass by pointer)
 *   - values: Array of String pointers (pass by pointer)
 *   - count: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_push_string_batch(
    uint64_t writer_handle,
    const uint64_t* timestamps_ns,
    const char* const* values,
    size_t count
);

/* ============================================================================
 * Lifecycle Control Functions
 * ========================================================================= */

/**
 * Flush all pending data for a stream
 * 
 * Forces immediate upload of all buffered data to Nominal Core or file.
 * Blocks until flush completes.
 * 
 * @param stream_handle Stream handle from nominal_init()
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * Example (LabVIEW):
 *   - stream_handle: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_flush(uint64_t stream_handle);

/**
 * Flush pending data for a specific channel
 * 
 * Forces immediate upload of buffered data for this channel.
 * Blocks until flush completes.
 * 
 * @param writer_handle Writer handle from nominal_create_channel()
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * Example (LabVIEW):
 *   - writer_handle: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_flush_channel(uint64_t writer_handle);

/* ============================================================================
 * Diagnostics & Monitoring Functions
 * ========================================================================= */

/**
 * Get the number of active stream handles
 * 
 * @return Number of active streams (>= 0)
 * 
 * Example (LabVIEW):
 *   - Return: I32 (by value)
 */
int32_t nominal_get_active_streams(void);

/**
 * Get the number of active writer handles
 * 
 * @return Number of active writers (>= 0)
 * 
 * Example (LabVIEW):
 *   - Return: I32 (by value)
 */
int32_t nominal_get_active_writers(void);

/**
 * Check if a stream handle is valid
 * 
 * @param stream_handle Stream handle to validate
 * 
 * @return 1 if valid, 0 if invalid
 * 
 * Example (LabVIEW):
 *   - stream_handle: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_is_stream_valid(uint64_t stream_handle);

/**
 * Check if a writer handle is valid
 * 
 * @param writer_handle Writer handle to validate
 * 
 * @return 1 if valid, 0 if invalid
 * 
 * Example (LabVIEW):
 *   - writer_handle: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_is_writer_valid(uint64_t writer_handle);

/* ============================================================================
 * Stream Information Functions
 * ========================================================================= */

/**
 * Get the channel name for a writer
 * 
 * @param writer_handle Writer handle from nominal_create_channel()
 * @param buffer [OUT] Buffer to receive channel name string
 * @param buffer_size Size of the buffer in bytes
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * Example (LabVIEW):
 *   - writer_handle: U64 (pass by value)
 *   - buffer: String (pass by pointer, pre-allocated)
 *   - buffer_size: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_get_channel_name(
    uint64_t writer_handle,
    char* buffer,
    size_t buffer_size
);

/**
 * Get library version information
 * 
 * @param buffer [OUT] Buffer to receive version string
 * @param buffer_size Size of the buffer in bytes
 * 
 * @return NOMINAL_SUCCESS on success, negative error code on failure
 * 
 * Example (LabVIEW):
 *   - buffer: String (pass by pointer, pre-allocated)
 *   - buffer_size: U64 (pass by value)
 *   - Return: I32 (by value)
 */
int32_t nominal_get_version(char* buffer, size_t buffer_size);

/* ============================================================================
 * LabVIEW Integration Notes
 * ========================================================================= */

/**
 * LABVIEW TIMESTAMP CONVERSION
 * 
 * LabVIEW timestamps are double-precision seconds since 1904-01-01 00:00:00 UTC
 * This library requires nanoseconds since Unix epoch (1970-01-01 00:00:00 UTC)
 * 
 * Conversion formula:
 *   timestamp_ns = (labview_timestamp - 2082844800.0) * 1e9
 * 
 * Where 2082844800.0 is the offset between 1904 and 1970 in seconds.
 * 
 * Example LabVIEW code:
 *   Timestamp Array -> Subtract 2082844800.0 -> Multiply 1e9 -> Convert to U64
 */

/**
 * LABVIEW CALL LIBRARY FUNCTION NODE CONFIGURATION
 * 
 * For string parameters:
 *   - Type: C String Pointer
 *   - Pass: Pointer
 *   - Null-terminated: Yes
 * 
 * For output parameters (out_stream_handle, out_writer_handle, buffer):
 *   - Pass: Pointer
 *   - Pre-allocate buffer for string outputs
 * 
 * For input arrays (timestamps_ns, values):
 *   - Pass: Pointer
 *   - Data type: Array of matching type (U64[], DBL[], I64[], etc.)
 * 
 * For count/size parameters:
 *   - Type: U64 (size_t)
 *   - Pass: By Value
 * 
 * For return values:
 *   - Type: I32
 *   - Pass: By Value
 */

#ifdef __cplusplus
}
#endif

#endif /* NOMINAL_LABVIEW_FFI_H */