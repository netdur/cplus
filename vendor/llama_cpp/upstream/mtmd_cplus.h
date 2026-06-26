#ifndef CPLUS_MTMD_CPLUS_H
#define CPLUS_MTMD_CPLUS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// Curated C header for cpc-bindgen over llama.cpp's tools/mtmd/mtmd.h.
// `mtmd.h` is a C API but explicitly experimental; keep this surface narrow.

enum mtmd_input_chunk_type {
    MTMD_INPUT_CHUNK_TYPE_TEXT,
    MTMD_INPUT_CHUNK_TYPE_IMAGE,
    MTMD_INPUT_CHUNK_TYPE_AUDIO,
};

typedef struct mtmd_input_text {
    const char * text;
    bool add_special;
    bool parse_special;
} mtmd_input_text;

struct mtmd_context_params {
    bool use_gpu;
    bool print_timings;
    int n_threads;
    const char * image_marker;
    const char * media_marker;
    int32_t flash_attn_type;
    bool warmup;
    int image_min_tokens;
    int image_max_tokens;
    void * cb_eval;
    void * cb_eval_user_data;
};

struct mtmd_decoder_pos {
    uint32_t t;
    uint32_t x;
    uint32_t y;
    uint32_t z;
};

struct mtmd_caps {
    bool inp_vision;
    bool inp_audio;
};

const char * mtmd_default_marker(void);
struct mtmd_context_params mtmd_context_params_default(void);

void * mtmd_init_from_file(const char * mmproj_fname, const void * text_model, struct mtmd_context_params ctx_params);
void mtmd_free(void * ctx);

bool mtmd_decode_use_non_causal(const void * ctx, const void * chunk);
bool mtmd_decode_use_mrope(const void * ctx);
bool mtmd_support_vision(const void * ctx);
bool mtmd_support_audio(const void * ctx);
int mtmd_get_audio_sample_rate(const void * ctx);
const char * mtmd_get_marker(const void * ctx);

void * mtmd_bitmap_init(uint32_t nx, uint32_t ny, const unsigned char * data);
void * mtmd_bitmap_init_from_audio(size_t n_samples, const float * data);
uint32_t mtmd_bitmap_get_nx(const void * bitmap);
uint32_t mtmd_bitmap_get_ny(const void * bitmap);
const unsigned char * mtmd_bitmap_get_data(const void * bitmap);
size_t mtmd_bitmap_get_n_bytes(const void * bitmap);
bool mtmd_bitmap_is_audio(const void * bitmap);
void mtmd_bitmap_free(void * bitmap);
const char * mtmd_bitmap_get_id(const void * bitmap);
void mtmd_bitmap_set_id(void * bitmap, const char * id);

void * mtmd_input_chunks_init(void);
size_t mtmd_input_chunks_size(const void * chunks);
const void * mtmd_input_chunks_get(const void * chunks, size_t idx);
void mtmd_input_chunks_free(void * chunks);

int32_t mtmd_input_chunk_get_type(const void * chunk);
const int32_t * mtmd_input_chunk_get_tokens_text(const void * chunk, size_t * n_tokens_output);
const void * mtmd_input_chunk_get_tokens_image(const void * chunk);
size_t mtmd_input_chunk_get_n_tokens(const void * chunk);
const char * mtmd_input_chunk_get_id(const void * chunk);
int32_t mtmd_input_chunk_get_n_pos(const void * chunk);
void * mtmd_input_chunk_copy(const void * chunk);
void mtmd_input_chunk_free(void * chunk);

size_t mtmd_image_tokens_get_n_tokens(const void * image_tokens);
const char * mtmd_image_tokens_get_id(const void * image_tokens);
int32_t mtmd_image_tokens_get_n_pos(const void * image_tokens);
struct mtmd_decoder_pos mtmd_image_tokens_get_decoder_pos(const void * image_tokens, int32_t pos_0, size_t i);

int32_t mtmd_tokenize(
    void * ctx,
    void * output,
    const mtmd_input_text * text,
    const void ** bitmaps,
    size_t n_bitmaps);

int32_t mtmd_encode(void * ctx, const void * image_tokens);
int32_t mtmd_encode_chunk(void * ctx, const void * chunk);
float * mtmd_get_output_embd(void * ctx);

struct mtmd_caps mtmd_get_cap_from_file(const char * mmproj_fname);

#endif
