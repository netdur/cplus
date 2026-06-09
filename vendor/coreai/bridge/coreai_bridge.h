#ifndef CPLUS_COREAI_BRIDGE_H
#define CPLUS_COREAI_BRIDGE_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

int32_t cplus_coreai_runtime_available(void);
int32_t cplus_coreai_last_error(uint8_t *buf, size_t len);
void cplus_coreai_release(void *handle);

void *cplus_coreai_model_load(const uint8_t *path, size_t path_len);
void *cplus_coreai_model_load_function(void *model, const uint8_t *name, size_t name_len);

void *cplus_coreai_ndarray_create_f32(
    const uint64_t *shape,
    uint64_t rank,
    const float *data,
    uint64_t count
);

void *cplus_coreai_function_run1_f32(
    void *function,
    const uint8_t *input_name,
    size_t input_name_len,
    void *input,
    const uint8_t *output_name,
    size_t output_name_len
);

int64_t cplus_coreai_ndarray_copy_f32(void *array, float *dest, uint64_t count);

#ifdef __cplusplus
}
#endif

#endif
