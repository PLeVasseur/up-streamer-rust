/* Copyright (c) 2026 Contributors to the Eclipse Foundation
 * SPDX-License-Identifier: Apache-2.0
 */
/* Prospective Linux/glibc test observer. Derived from the qualified LoLa test
 * interposer. Allocation entries are charged once before unchanged glibc
 * delegation. Counters include Rust System and native C/C++ allocation calls;
 * observer costs are never subtracted. No production loader configuration is
 * changed: a test launcher explicitly supplies LD_PRELOAD and a private file.
 * This prepared source has not yet been qualified or integrated.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

extern void *__libc_malloc(size_t);
extern void *__libc_calloc(size_t, size_t);
extern void *__libc_realloc(void *, size_t);
extern void *__libc_memalign(size_t, size_t);
extern void __libc_free(void *);

/* All words have fixed u64 storage. The native byte order and sizeof structure
 * are recorded in the header and checked by the independent reader. A local
 * spin lock serializes writers; an even sequence brackets a coherent snapshot.
 * Lane counters remain enabled from actor construction through exit; readiness
 * and phase snapshots define windows without resetting or subtracting costs.
 */
struct counters {
    uint64_t magic;
    uint64_t version;
    uint64_t byte_size;
    uint64_t pid;
    _Atomic uint64_t sequence;
    _Atomic uint64_t enabled;
    _Atomic uint64_t process_calls;
    _Atomic uint64_t process_bytes;
    _Atomic uint64_t binding_calls;
    _Atomic uint64_t binding_bytes;
    _Atomic uint64_t malloc_calls;
    _Atomic uint64_t calloc_calls;
    _Atomic uint64_t realloc_calls;
    _Atomic uint64_t aligned_calls;
    _Atomic uint64_t scope_entries;
    _Atomic uint64_t scope_threads;
    _Atomic uint64_t overflow;
    uint64_t pointer_bits;
    _Atomic uint64_t process_threads;
};
_Static_assert(sizeof(struct counters) == 152, "observer v1 fixed header");
_Static_assert(sizeof(void *) == 8 && ATOMIC_LONG_LOCK_FREE == 2 && ATOMIC_LLONG_LOCK_FREE == 2,
               "observer requires native lock-free 64-bit counters");
static struct counters *observed = NULL;
static atomic_flag writer = ATOMIC_FLAG_INIT;
/* This library is loaded only by the ELF loader at actor startup. Initial-exec
 * TLS avoids a dynamic TLS lookup/allocation recursively entering malloc hooks.
 */
static _Thread_local unsigned int binding_scope __attribute__((tls_model("initial-exec"))) = 0;
static _Thread_local unsigned int scope_seen __attribute__((tls_model("initial-exec"))) = 0;
static _Thread_local unsigned int allocation_seen __attribute__((tls_model("initial-exec"))) = 0;
static void lock(void) {
    while (atomic_flag_test_and_set_explicit(&writer, memory_order_acquire)) {
        atomic_signal_fence(memory_order_seq_cst);
    }
    atomic_fetch_add_explicit(&observed->sequence, 1, memory_order_acq_rel);
}
static void unlock(void) {
    atomic_fetch_add_explicit(&observed->sequence, 1, memory_order_release);
    atomic_flag_clear_explicit(&writer, memory_order_release);
}
static void add(_Atomic uint64_t *value, uint64_t amount) {
    uint64_t before = atomic_load_explicit(value, memory_order_relaxed);
    if (amount > UINT64_MAX - before) {
        atomic_store_explicit(value, UINT64_MAX, memory_order_relaxed);
        atomic_store_explicit(&observed->overflow, 1, memory_order_relaxed);
    } else {
        atomic_store_explicit(value, before + amount, memory_order_relaxed);
    }
}
enum kind { MALLOC, CALLOC, REALLOC, ALIGNED };
static void charge(size_t size, enum kind kind) {
    if (observed == NULL || !atomic_load_explicit(&observed->enabled, memory_order_acquire)) return;
    lock();
    /* A stop racing this entry defines the measurement boundary. Calls already
     * inside the writer are included; later entries observe disabled and leave.
     */
    if (atomic_load_explicit(&observed->enabled, memory_order_acquire)) {
        add(&observed->process_calls, 1);
        add(&observed->process_bytes, size);
        if (!allocation_seen) { allocation_seen = 1; add(&observed->process_threads, 1); }
        if (binding_scope) {
            add(&observed->binding_calls, 1);
            add(&observed->binding_bytes, size);
        }
        switch (kind) {
        case MALLOC: add(&observed->malloc_calls, 1); break;
        case CALLOC: add(&observed->calloc_calls, 1); break;
        case REALLOC: add(&observed->realloc_calls, 1); break;
        case ALIGNED: add(&observed->aligned_calls, 1); break;
        }
    }
    unlock();
}
__attribute__((constructor)) static void initialize(void) {
    const char *path = getenv("UPROTOCOL_ALLOCATION_COUNTER_FILE");
    if (path == NULL || *path == '\0') return;
    int fd = open(path, O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (fd < 0 || ftruncate(fd, 4096) != 0) _exit(121);
    void *mapped = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    close(fd);
    if (mapped == MAP_FAILED) _exit(122);
    struct counters *value = mapped;
    value->version = 1;
    value->byte_size = sizeof(struct counters);
    value->pid = (uint64_t)getpid();
    value->pointer_bits = sizeof(void *) * CHAR_BIT;
#define INIT(name) atomic_init(&value->name, 0)
    INIT(sequence); INIT(enabled); INIT(process_calls); INIT(process_bytes);
    INIT(binding_calls); INIT(binding_bytes); INIT(malloc_calls); INIT(calloc_calls);
    INIT(realloc_calls); INIT(aligned_calls); INIT(scope_entries); INIT(scope_threads);
    INIT(overflow); INIT(process_threads);
#undef INIT
    atomic_store_explicit(&value->enabled, 1, memory_order_relaxed);
    value->magic = UINT64_C(0x3130434f4c415055); /* UPALOC01, little-endian host */
    observed = value;
}
unsigned int up_flow_alloc_observer_version(void) { return 1; }
unsigned int up_flow_alloc_observer_mapped(void) { return observed != NULL; }
unsigned int up_flow_alloc_replace_scope(unsigned int active) {
    unsigned int previous = binding_scope;
    binding_scope = active != 0;
    if (binding_scope && observed != NULL && atomic_load_explicit(&observed->enabled, memory_order_acquire)) {
        lock();
        add(&observed->scope_entries, 1);
        if (!scope_seen) { scope_seen = 1; add(&observed->scope_threads, 1); }
        unlock();
    }
    return previous;
}
/* Used only by isolated calibration; actual actors keep monotonic counters on. */
void up_flow_alloc_set_enabled(unsigned int active) {
    if (observed != NULL) atomic_store_explicit(&observed->enabled, active != 0, memory_order_release);
}
void *malloc(size_t size) { charge(size, MALLOC); return __libc_malloc(size); }
void *calloc(size_t count, size_t size) {
    charge(count != 0 && size > SIZE_MAX / count ? SIZE_MAX : count * size, CALLOC);
    return __libc_calloc(count, size);
}
void *realloc(void *pointer, size_t size) { charge(size, REALLOC); return __libc_realloc(pointer, size); }
void free(void *pointer) { __libc_free(pointer); }
void *memalign(size_t alignment, size_t size) { charge(size, ALIGNED); return __libc_memalign(alignment, size); }
void *aligned_alloc(size_t alignment, size_t size) { charge(size, ALIGNED); return __libc_memalign(alignment, size); }
int posix_memalign(void **output, size_t alignment, size_t size) {
    charge(size, ALIGNED);
    if (alignment < sizeof(void *) || (alignment & (alignment - 1)) != 0) return EINVAL;
    void *pointer = __libc_memalign(alignment, size);
    if (!pointer) return ENOMEM;
    *output = pointer;
    return 0;
}
void *valloc(size_t size) { charge(size, ALIGNED); return __libc_memalign((size_t)getpagesize(), size); }
void *pvalloc(size_t size) {
    size_t page = (size_t)getpagesize();
    if (size > SIZE_MAX - (page - 1)) { charge(SIZE_MAX, ALIGNED); errno = ENOMEM; return NULL; }
    size_t rounded = (size + page - 1) & ~(page - 1);
    charge(rounded, ALIGNED);
    return __libc_memalign(page, rounded);
}
void *reallocarray(void *pointer, size_t count, size_t size) {
    if (count != 0 && size > SIZE_MAX / count) { charge(SIZE_MAX, REALLOC); errno = ENOMEM; return NULL; }
    size_t total = count * size;
    charge(total, REALLOC);
    return __libc_realloc(pointer, total);
}
