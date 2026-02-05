#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <time.h>

// Function declarations
int32_t nominal_init(
    const char* token,
    const char* dataset_rid,
    const char* fallback_file_path,
    uint64_t* out_stream_handle
);

int32_t nominal_create_channel(
    uint64_t stream_handle,
    const char* channel_name,
    const char* tags_csv,
    uint64_t* out_writer_handle
);

int32_t nominal_push_double_batch(
    uint64_t writer_handle,
    const uint64_t* timestamps_ns,
    const double* values,
    size_t count
);

int32_t nominal_close_channel(uint64_t writer_handle);
int32_t nominal_shutdown(uint64_t stream_handle);
int32_t nominal_get_last_error(char* buffer, size_t buffer_size);

int main() {
    printf("Nominal FFI Test Program\n");
    printf("========================\n\n");

    uint64_t stream_handle = 0;
    uint64_t writer_handle = 0;
    char error_buf[256];
    int32_t result;

    // Initialize stream
    printf("1. Initializing stream...\n");
    result = nominal_init(
        NULL,  // Use NOMINAL_TOKEN from env
        "ri.dataset.main.dataset.your-dataset-rid",
        "/tmp/nominal_fallback.avro",
        &stream_handle
    );
    
    if (result != 0) {
        nominal_get_last_error(error_buf, sizeof(error_buf));
        printf("   ERROR: %s\n", error_buf);
        return 1;
    }
    printf("   SUCCESS! Stream handle: %llu\n\n", stream_handle);

    // Create channel
    printf("2. Creating channel...\n");
    result = nominal_create_channel(
        stream_handle,
        "temperature",
        "experiment=test,sensor=front",
        &writer_handle
    );
    
    if (result != 0) {
        nominal_get_last_error(error_buf, sizeof(error_buf));
        printf("   ERROR: %s\n", error_buf);
        nominal_shutdown(stream_handle);
        return 1;
    }
    printf("   SUCCESS! Writer handle: %llu\n\n", writer_handle);

    // Push batch of data
    printf("3. Pushing batch of 100 data points...\n");
    
    const size_t count = 100;
    uint64_t timestamps[count];
    double values[count];
    
    // Current time in nanoseconds
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    uint64_t base_ns = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
    
    // Generate synthetic data
    for (size_t i = 0; i < count; i++) {
        timestamps[i] = base_ns + i * 1000000ULL;  // 1ms intervals
        values[i] = 20.0 + (i % 10) * 0.5;         // Varies between 20-25°C
    }
    
    result = nominal_push_double_batch(writer_handle, timestamps, values, count);
    
    if (result != 0) {
        nominal_get_last_error(error_buf, sizeof(error_buf));
        printf("   ERROR: %s\n", error_buf);
        nominal_close_channel(writer_handle);
        nominal_shutdown(stream_handle);
        return 1;
    }
    printf("   SUCCESS! Pushed %zu points\n\n", count);

    // Close channel
    printf("4. Closing channel...\n");
    result = nominal_close_channel(writer_handle);
    
    if (result != 0) {
        nominal_get_last_error(error_buf, sizeof(error_buf));
        printf("   ERROR: %s\n", error_buf);
    } else {
        printf("   SUCCESS!\n\n");
    }

    // Shutdown stream
    printf("5. Shutting down stream...\n");
    result = nominal_shutdown(stream_handle);
    
    if (result != 0) {
        nominal_get_last_error(error_buf, sizeof(error_buf));
        printf("   ERROR: %s\n", error_buf);
    } else {
        printf("   SUCCESS!\n\n");
    }

    printf("All tests completed!\n");
    return 0;
}