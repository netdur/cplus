#ifndef CPLUS_LLAMA_CPLUS_H
#define CPLUS_LLAMA_CPLUS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// Curated C header for cpc-bindgen.
//
// This intentionally does not include upstream llama.h. The exported symbols
// and ABI shapes match llama.h, but opaque upstream handles are represented as
// `void *` so the generated C+ does not have to model ggml internals.

typedef struct llama_batch {
    int32_t n_tokens;
    int32_t      *  token;
    float        *  embd;
    int32_t      *  pos;
    int32_t      *  n_seq_id;
    int32_t      ** seq_id;
    int8_t       *  logits;
} llama_batch;

struct llama_model_params {
    void * devices;
    const void * tensor_buft_overrides;
    int32_t n_gpu_layers;
    int32_t split_mode;
    int32_t main_gpu;
    const float * tensor_split;
    void * progress_callback;
    void * progress_callback_user_data;
    const void * kv_overrides;
    bool vocab_only;
    bool use_mmap;
    bool use_direct_io;
    bool use_mlock;
    bool check_tensors;
    bool use_extra_bufts;
    bool no_host;
    bool no_alloc;
};

struct llama_sampler_seq_config {
    int32_t seq_id;
    void * sampler;
};

struct llama_context_params {
    uint32_t n_ctx;
    uint32_t n_batch;
    uint32_t n_ubatch;
    uint32_t n_seq_max;
    uint32_t n_rs_seq;
    uint32_t n_outputs_max;
    int32_t n_threads;
    int32_t n_threads_batch;
    int32_t ctx_type;
    int32_t rope_scaling_type;
    int32_t pooling_type;
    int32_t attention_type;
    int32_t flash_attn_type;
    float rope_freq_base;
    float rope_freq_scale;
    float yarn_ext_factor;
    float yarn_attn_factor;
    float yarn_beta_fast;
    float yarn_beta_slow;
    uint32_t yarn_orig_ctx;
    float defrag_thold;
    void * cb_eval;
    void * cb_eval_user_data;
    int32_t type_k;
    int32_t type_v;
    void * abort_callback;
    void * abort_callback_data;
    bool embeddings;
    bool offload_kqv;
    bool no_perf;
    bool op_offload;
    bool swa_full;
    bool kv_unified;
    struct llama_sampler_seq_config * samplers;
    size_t n_samplers;
    void * ctx_other;
};

typedef struct llama_sampler_chain_params {
    bool no_perf;
} llama_sampler_chain_params;

typedef struct llama_chat_message {
    const char * role;
    const char * content;
} llama_chat_message;

typedef struct llama_token_data {
    int32_t id;
    float logit;
    float p;
} llama_token_data;

typedef struct llama_token_data_array {
    llama_token_data * data;
    size_t size;
    int64_t selected;
    bool sorted;
} llama_token_data_array;

struct llama_model_params llama_model_default_params(void);
struct llama_context_params llama_context_default_params(void);
struct llama_sampler_chain_params llama_sampler_chain_default_params(void);

void llama_backend_init(void);
void llama_backend_free(void);

void * llama_model_load_from_file(const char * path_model, struct llama_model_params params);
void llama_model_free(void * model);

void * llama_init_from_model(void * model, struct llama_context_params params);
void llama_free(void * ctx);

uint32_t llama_n_ctx(const void * ctx);
uint32_t llama_n_batch(const void * ctx);
uint32_t llama_n_ubatch(const void * ctx);

const void * llama_get_model(const void * ctx);
const void * llama_model_get_vocab(const void * model);

int32_t llama_model_n_ctx_train(const void * model);
int32_t llama_model_n_embd(const void * model);
int32_t llama_model_n_embd_inp(const void * model);
int32_t llama_model_n_embd_out(const void * model);
int32_t llama_model_n_layer(const void * model);
int32_t llama_model_n_head(const void * model);
int32_t llama_vocab_n_tokens(const void * vocab);

int32_t llama_model_desc(const void * model, char * buf, size_t buf_size);
uint64_t llama_model_size(const void * model);
uint64_t llama_model_n_params(const void * model);
const char * llama_model_chat_template(const void * model, const char * name);

llama_batch llama_batch_init(int32_t n_tokens, int32_t embd, int32_t n_seq_max);
llama_batch llama_batch_get_one(int32_t * tokens, int32_t n_tokens);
void llama_batch_free(llama_batch batch);
int32_t llama_encode(void * ctx, llama_batch batch);
int32_t llama_decode(void * ctx, llama_batch batch);

void llama_set_n_threads(void * ctx, int32_t n_threads, int32_t n_threads_batch);
void llama_synchronize(void * ctx);
float * llama_get_logits(void * ctx);
float * llama_get_logits_ith(void * ctx, int32_t i);
float * llama_get_embeddings(void * ctx);
float * llama_get_embeddings_ith(void * ctx, int32_t i);

const char * llama_vocab_get_text(const void * vocab, int32_t token);
bool llama_vocab_is_eog(const void * vocab, int32_t token);
bool llama_vocab_is_control(const void * vocab, int32_t token);
int32_t llama_vocab_bos(const void * vocab);
int32_t llama_vocab_eos(const void * vocab);
int32_t llama_vocab_eot(const void * vocab);
int32_t llama_vocab_nl(const void * vocab);
bool llama_vocab_get_add_bos(const void * vocab);
bool llama_vocab_get_add_eos(const void * vocab);

int32_t llama_tokenize(
    const void * vocab,
    const char * text,
    int32_t text_len,
    int32_t * tokens,
    int32_t n_tokens_max,
    bool add_special,
    bool parse_special);

int32_t llama_token_to_piece(
    const void * vocab,
    int32_t token,
    char * buf,
    int32_t length,
    int32_t lstrip,
    bool special);

int32_t llama_detokenize(
    const void * vocab,
    const int32_t * tokens,
    int32_t n_tokens,
    char * text,
    int32_t text_len_max,
    bool remove_special,
    bool unparse_special);

int32_t llama_chat_apply_template(
    const char * tmpl,
    const llama_chat_message * chat,
    size_t n_msg,
    bool add_ass,
    char * buf,
    int32_t length);

void * llama_sampler_chain_init(struct llama_sampler_chain_params params);
void llama_sampler_chain_add(void * chain, void * smpl);
void llama_sampler_free(void * smpl);
void * llama_sampler_init_greedy(void);
void * llama_sampler_init_dist(uint32_t seed);
void * llama_sampler_init_top_k(int32_t k);
void * llama_sampler_init_top_p(float p, size_t min_keep);
void * llama_sampler_init_min_p(float p, size_t min_keep);
void * llama_sampler_init_temp(float t);
void llama_sampler_accept(void * smpl, int32_t token);
int32_t llama_sampler_sample(void * smpl, void * ctx, int32_t idx);

#endif
