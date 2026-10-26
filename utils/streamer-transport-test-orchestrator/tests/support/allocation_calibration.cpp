// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
// Native C/C++ positive controls for every charged allocator family and threads.
#include <cstdlib>
#include <cstring>
#include <dlfcn.h>
#include <new>
#include <thread>
#include <vector>
static thread_local volatile unsigned char sink;
using Replace = unsigned int (*)(unsigned int);
static void exercise(Replace scope) {
    const auto previous = scope(1);
    void *p = malloc(37); if (!p) std::abort(); memset(p, 3, 37);
    p = realloc(p, 91); if (!p) std::abort(); sink = static_cast<unsigned char *>(p)[0]; free(p);
    p = calloc(3, 23); if (!p) std::abort(); sink = static_cast<unsigned char *>(p)[0]; free(p);
    if (posix_memalign(&p, 64, 128) != 0) std::abort(); memset(p, 4, 128); sink = static_cast<unsigned char *>(p)[0]; free(p);
    auto *array = new unsigned char[173]; memset(array, 5, 173); sink = array[7]; delete[] array;
    scope(previous);
}
int main() {
    auto scope = reinterpret_cast<Replace>(dlsym(RTLD_DEFAULT, "up_flow_alloc_replace_scope"));
    if (!scope) return 9;
    std::vector<std::thread> workers;
    for (unsigned i = 0; i < 4; ++i) workers.emplace_back([scope] { exercise(scope); });
    for (auto &worker : workers) worker.join();
    exercise(scope);
    return 0;
}
