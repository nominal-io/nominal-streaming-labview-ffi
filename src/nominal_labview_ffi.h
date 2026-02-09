/**
 * Nominal Streaming FFI Library for LabVIEW
 * 
 * This header defines the C interface for the Rust-based Nominal streaming library.
 * All functions are thread-safe and can be called from multiple LabVIEW VIs concurrently.
 * 
 * Typical usage pattern:
 * 1. Call nominal_init() once at startup to create a stream
 * 2. Call nominal_create_channel() for each data channel you want to write
 * 3. Call nominal_push_double_batch() repeatedly to send data
 * 4. Call nominal_close_channel() when done with a channel
 * 5. Call nominal_shutdown() at program exit to flush and cleanup
 * 
 * Error handling:
 * - All functions return 0 on success, negative error codes on failure
 * - Call nominal_get_last_error() to retrieve detailed error messages
 */

#ifndef NOMINAL_LABVIEW_FFI_H
#define NOMINAL_LABVIEW_FFI_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stdint.h>
#include <stddef.h>

/* ============================================================================
 * Error Codes
 * ============================================================================ */

#define NOMINAL_SUCCESS              0
#define NOMINAL_ERROR_GENERIC       -1
#define NOMINAL_ERROR_INVALID_HANDLE -2
#define NOMINAL_ERROR_INVALID_PARAM  -3
#define NOMINAL_ERROR_RUNTIME        -4
#define NOMINAL_ERROR_IO             -5

/* ============================================================================
 * Handle Types
 * ============================================================================ */

typedef uint64_t NominalStreamHandle;
typedef uint64_t NominalWriterHandle;

/* ============================================================================
 * Core Functions
 * ============================================================================ */

/**
 * Initialize a new Nominal stream.
 * 
 * This function creates a streaming connection to Nominal Core and/or a local
 * AVRO file for data fallback. At least one of token (or NOMINAL_TOKEN env var)
 * or fallback_file_path must be provided.
 * 
 * @param token               Nominal API token (can be NULL to use NOMINAL_TOKEN env var)
 * @param dataset_rid         Dataset Resource Identifier (e.g., "ri.catalog.main.dataset.abc123")
 * @param fallback_file_path  Path for fallback AVRO file (can be NULL for no fallback)
 * @param out_stream_handle   [OUT] Pointer to receive the stream handle
 * 
 * @return NOMINAL_SUCCESS (0) on success, negative error code on failure
 * 
 * @note This function spawns background threads for async I/O. The Tokio runtime
 *       is initialized on first call and persists for the lifetime of the process.
 * 
 * Example (LabVIEW Call Library Function Node configuration):
 *   - Function name: nominal_init
 *   - Return type: int32_t
 *   - Parameters:
 *     * token: string pointer (const char*) - Pass By Value
 *     * dataset_rid: string pointer (const char*) - Pass By Value
 *     * fallback_file_path: string pointer (const char*) - Pass By Value
 *     * out_stream_handle: uint64_t pointer - Pass By Reference
 */
int32_t nominal_init(
    const char* token,
    const char* dataset_rid,
    const char* fallback_file_path,
    uint64_t* out_stream_handle
);

/**
 * Create a channel writer for a specific data channel.
 * 
 * Channels are identified by name and optional tags. Tags allow you to differentiate
 * multiple instances of the same channel (e.g., multiple sensors of the same type).
 * 
 * @param stream_handle      Stream handle from nominal_init()
 * @param channel_name       Name of the channel (e.g., "temperature", "pressure")
 * @param tags_csv           Comma-separated key=value pairs (e.g., "exp=123,sensor=front")
 *                           Can be NULL or empty string for no tags
 * @param out_writer_handle  [OUT] Pointer to receive the writer handle
 * 
 * @return NOMINAL_SUCCESS (0) on success, negative error code on failure
 * 
 * Example (LabVIEW):
 *   - Function name: nominal_create_channel
 *   - Return type: int32_t
 *   - Parameters:
 *     * stream_handle: uint64_t - Pass By Value
 *     * channel_name: string pointer (const char*) - Pass By Value
 *     * tags_csv: string pointer (const char*) - Pass By Value
 *     * out_writer_handle: uint64_t pointer - Pass By Reference
 */
int32_t nominal_create_channel(
    uint64_t stream_handle,
    const char* channel_name,
    const char* tags_csv,
    uint64_t* out_writer_handle
);

/**
 * Push a batch of double-precision data points to a channel.
 * 
 * This is the main data ingestion function. It accepts arrays of timestamps and
 * values and efficiently streams them to Nominal. The function is optimized for
 * batch operation - sending 100-1000 points per call is more efficient than
 * individual points.
 * 
 * @param writer_handle  Writer handle from nominal_create_channel()
 * @param timestamps_ns  Array of timestamps in nanoseconds since Unix epoch (Jan 1, 1970)
 * @param values         Array of double-precision values
 * @param count          Number of points in both arrays (must match)
 * 
 * @return NOMINAL_SUCCESS (0) on success, negative error code on failure
 * 
 * @note Timestamps must be in nanoseconds since Unix epoch. To convert LabVIEW
 *       timestamps (seconds since 1904), use:
 *       unix_ns = (lv_timestamp - 2082844800.0) * 1e9
 * 
 * Example (LabVIEW):
 *   - Function name: nominal_push_double_batch
 *   - Return type: int32_t
 *   - Parameters:
 *     * writer_handle: uint64_t - Pass By Value
 *     * timestamps_ns: uint64_t array pointer - Pass By Reference
 *     * values: double array pointer - Pass By Reference
 *     * count: size_t (uintptr on 32-bit, uint64 on 64-bit) - Pass By Value
 */
int32_t nominal_push_double_batch(
    uint64_t writer_handle,
    const uint64_t* timestamps_ns,
    const double* values,
    size_t count
);

/**
 * Close a channel writer and flush any remaining buffered data.
 * 
 * This function should be called when you're done writing to a channel.
 * It ensures all buffered data is flushed to Nominal and releases resources.
 * 
 * @param writer_handle  Writer handle from nominal_create_channel()
 * 
 * @return NOMINAL_SUCCESS (0) on success, negative error code on failure
 * 
 * @note The writer handle becomes invalid after this call and should not be used again.
 * 
 * Example (LabVIEW):
 *   - Function name: nominal_close_channel
 *   - Return type: int32_t
 *   - Parameters:
 *     * writer_handle: uint64_t - Pass By Value
 */
int32_t nominal_close_channel(uint64_t writer_handle);

/**
 * Shutdown the stream and cleanup all resources.
 * 
 * This function flushes all remaining data, closes network connections, and
 * releases all resources associated with the stream. It should be called once
 * at program shutdown after all channels have been closed.
 * 
 * @param stream_handle  Stream handle from nominal_init()
 * 
 * @return NOMINAL_SUCCESS (0) on success, negative error code on failure
 * 
 * @note The stream handle becomes invalid after this call. Any writer handles
 *       associated with this stream should have already been closed.
 * 
 * Example (LabVIEW):
 *   - Function name: nominal_shutdown
 *   - Return type: int32_t
 *   - Parameters:
 *     * stream_handle: uint64_t - Pass By Value
 */
int32_t nominal_shutdown(uint64_t stream_handle);

/**
 * Get the last error message from the most recent failed operation.
 * 
 * Error messages are stored in thread-local storage, so each thread has its
 * own error message. This function retrieves the error message for the current
 * thread and clears it.
 * 
 * @param buffer       [OUT] Buffer to receive the error message (null-terminated string)
 * @param buffer_size  Size of the buffer in bytes
 * 
 * @return 0 if error message was retrieved successfully
 *         -1 if no error is stored
 *         -3 if buffer is NULL or buffer_size is 0
 * 
 * @note Recommended buffer size is at least 256 bytes. If the error message is
 *       longer than buffer_size-1, it will be truncated.
 * 
 * Example (LabVIEW):
 *   - Function name: nominal_get_last_error
 *   - Return type: int32_t
 *   - Parameters:
 *     * buffer: string pointer (char*) - Pass By Reference
 *     * buffer_size: size_t (uintptr on 32-bit, uint64 on 64-bit) - Pass By Value
 */
int32_t nominal_get_last_error(
    char* buffer,
    size_t buffer_size
);

/* ============================================================================
 * LabVIEW Integration Notes
 * ============================================================================ */

/*
 * CRITICAL: Parameter Passing Convention
 * 
 * LabVIEW Call Library Function Node requires careful parameter configuration:
 * 
 * PASS BY VALUE (input parameters):
 *   - All scalar types: int32_t, uint64_t, size_t
 *   - String inputs: const char* (configure as "String Handle" in LabVIEW)
 * 
 * PASS BY REFERENCE (output parameters and arrays):
 *   - Output handles: uint64_t* (LabVIEW wire connected to U64 indicator)
 *   - Arrays: uint64_t*, double* (LabVIEW arrays)
 *   - Output buffer: char* (LabVIEW string for error messages)
 * 
 * TIMESTAMP CONVERSION:
 * 
 * LabVIEW timestamps are doubles representing seconds since 1904-01-01 00:00:00 UTC.
 * Nominal expects nanoseconds since Unix epoch (1970-01-01 00:00:00 UTC).
 * 
 * Conversion formula:
 *   unix_ns = (lv_timestamp - 2082844800.0) * 1000000000.0
 * 
 * Where:
 *   - 2082844800.0 = seconds between 1904 and 1970
 *   - 1000000000.0 = nanoseconds per second
 * 
 * THREADING:
 * 
 * All functions are thread-safe and can be called from multiple VIs concurrently.
 * However, be aware that the Tokio runtime spawns background threads that will
 * continue running until the process exits.
 * 
 * PLATFORM-SPECIFIC NOTES:
 * 
 * Windows (32-bit LabVIEW):
 *   - Library: nominal_labview_ffi.dll (32-bit build required)
 *   - Calling convention: C (stdcall on Windows is NOT used)
 *   - Size_t is 32-bit (use U32 in LabVIEW)
 * 
 * Windows (64-bit LabVIEW):
 *   - Library: nominal_labview_ffi.dll (64-bit build required)
 *   - Size_t is 64-bit (use U64 in LabVIEW)
 * 
 * Linux:
 *   - Library: libnominal_labview_ffi.so
 *   - Size_t is platform-specific (32 or 64-bit)
 * 
 * macOS:
 *   - Library: libnominal_labview_ffi.dylib
 *   - Size_t is 64-bit on modern Macs
 * 
 * RT Targets (NI Linux RT):
 *   - Library: libnominal_labview_ffi.so (cross-compiled for ARM)
 *   - Deploy to /usr/local/lib/ or bundle with VI
 *   - Memory-constrained: monitor usage carefully
 *   - Must call from non-RT loop to avoid blocking deterministic code
 */

#ifdef __cplusplus
}
#endif

#endif /* NOMINAL_LABVIEW_FFI_H */