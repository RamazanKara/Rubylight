// SPDX-License-Identifier: BSD-2-Clause
// Copyright © 2021 Billy Laws

#include <array>
#include <cstdio>
#include <cstdlib>
#include <errno.h>
#include <dlfcn.h>
#include <unistd.h>
#include <fcntl.h>
#include <android/dlext.h>
#include <android/log.h>
#include <android/api-level.h>
#include <sys/mman.h>
#include "elf_soname_patcher.h"
#include "android_linker_ns.h"

using loader_android_create_namespace_t = android_namespace_t *(*)(const char *, const char *, const char *, uint64_t, const char *, android_namespace_t *, const void *);
static loader_android_create_namespace_t loader_android_create_namespace;

static bool lib_loaded;


/* Public API */
bool linkernsbypass_load_status() {
    return lib_loaded;
}

struct android_namespace_t *android_create_namespace(const char *name,
                                                     const char *ld_library_path,
                                                     const char *default_library_path,
                                                     uint64_t type,
                                                     const char *permitted_when_isolated_path,
                                                     android_namespace_t *parent_namespace) {
    auto caller{__builtin_return_address(0)};
    return loader_android_create_namespace(name, ld_library_path, default_library_path, type,
                                           permitted_when_isolated_path, parent_namespace, caller);
}

struct android_namespace_t *android_create_namespace_escape(const char *name,
                                                            const char *ld_library_path,
                                                            const char *default_library_path,
                                                            uint64_t type,
                                                            const char *permitted_when_isolated_path,
                                                            android_namespace_t *parent_namespace) {
    auto caller{reinterpret_cast<void *>(&dlopen)};
    return loader_android_create_namespace(name, ld_library_path, default_library_path, type,
                                           permitted_when_isolated_path, parent_namespace, caller);
}

android_get_exported_namespace_t android_get_exported_namespace;

android_link_namespaces_all_libs_t android_link_namespaces_all_libs;

android_link_namespaces_t android_link_namespaces;

bool linkernsbypass_link_namespace_to_default_all_libs(android_namespace_t *to) {
    // Creating a shared namespace with the default parent will give a copy of the default namespace that we can actually access
    // This is needed since there is no way to access a direct handle to the default namespace as it's not exported
    static auto defaultNs{android_create_namespace_escape("default_copy", nullptr, nullptr, ANDROID_NAMESPACE_TYPE_SHARED, nullptr, nullptr)};
    return android_link_namespaces_all_libs(to, defaultNs);
}

void *linkernsbypass_namespace_dlopen(const char *filename, int flags, android_namespace_t *ns) {
    android_dlextinfo extInfo{
        .flags = ANDROID_DLEXT_USE_NAMESPACE,
        .library_namespace = ns
    };

    return android_dlopen_ext(filename, flags, &extInfo);
}

#ifndef __NR_memfd_create
    #if defined(__aarch64__)
        #define __NR_memfd_create 279
    #else
        #error Unsupported target architecture!
    #endif
#endif

void *linkernsbypass_namespace_dlopen_unique(const char *libPath, const char *libTargetDir, int flags, android_namespace_t *ns) {
    static std::array<char, PATH_MAX> PathBuf{};

    // Used as a unique ID for overwriting soname and creating target lib files
    static uint16_t TargetId{};

    int libTargetFd{[&] () {
        if (libTargetDir) {
            snprintf(PathBuf.data(), PathBuf.size(), "%s/%d_patched.so", libTargetDir, TargetId);
            return open(PathBuf.data(), O_CREAT | O_RDWR, S_IRUSR | S_IWUSR);
        } else {
            // If memfd isn't supported errno will contain ENOSYS after calling
            errno = 0;
            int fd{static_cast<int>(syscall(__NR_memfd_create, libPath, 0))};
            if (errno == ENOSYS || fd < 0)
                return -1;
            else
                return fd;
        }
    }()};
    if (libTargetFd == -1)
        return nullptr;

    // Leave room for every uint16_t ID and its terminator so distinct loads keep distinct SONAMEs.
    std::array<char, 6> sonameOverwrite{};
    snprintf(sonameOverwrite.data(), sonameOverwrite.size(), "%03u", TargetId++);

    if (!elf_soname_patch(libPath, libTargetFd, sonameOverwrite.data()))
        return nullptr;

    // Load our patched library into the hook namespace
    android_dlextinfo hookExtInfo{
        .flags = ANDROID_DLEXT_USE_NAMESPACE | ANDROID_DLEXT_USE_LIBRARY_FD,
        .library_fd = libTargetFd,
        .library_namespace = ns
    };

    // Make a path that looks about right
    snprintf(PathBuf.data(), PathBuf.size(), "/proc/self/fd/%d", libTargetFd);

    return android_dlopen_ext(PathBuf.data(), flags, &hookExtInfo);
}

static void *align_ptr(void *ptr) {
    return reinterpret_cast<void *>(reinterpret_cast<uintptr_t>(ptr) & ~(getpagesize() - 1));
}

/* Private */
__attribute__((constructor)) static void resolve_linker_symbols() {
    using loader_dlopen_t = void *(*)(const char *, int, const void *);

    if (android_get_device_api_level() < 28)
        return;

    // ARM64 specific function walking to locate the internal dlopen handler
    auto loader_dlopen{[]() -> loader_dlopen_t {
        auto page{align_ptr(reinterpret_cast<void *>(&dlopen))};
        const size_t pageSize{static_cast<size_t>(getpagesize())};
        // Nightjar: execute-only mappings must become readable before inspection.
        // If the linker layout differs, fail the optional driver load rather than scan other code.
        if (mprotect(page, pageSize, PROT_READ | PROT_EXEC) != 0)
            return nullptr;
        auto instruction{reinterpret_cast<const uint32_t *>(&dlopen)};
        const auto pageEnd{reinterpret_cast<uintptr_t>(page) + pageSize};
        for (size_t index{}; index < 32 && reinterpret_cast<uintptr_t>(instruction + index) < pageEnd; index++) {
            uint32_t code{instruction[index]};
            if ((code & 0xfc000000U) == 0x94000000U) {
                int64_t offset{code & 0x03ffffffU};
                if (offset & 0x02000000) offset -= 0x04000000;
                return reinterpret_cast<loader_dlopen_t>(
                        reinterpret_cast<uintptr_t>(instruction + index) + offset * 4);
            }
            if ((code & 0xfffffc1fU) == 0xd65f0000U || // RET
                (code & 0xfffffc1fU) == 0xd61f0000U || // BR
                (code & 0xfc000000U) == 0x14000000U || // B
                code == 0xd65f0bffU || code == 0xd65f0fffU) // RETAA / RETAB
                break;
        }
        return nullptr;
    }()};
    if (!loader_dlopen)
        return;

    // Protect the loader_dlopen function to remove the BTI attribute (since this is an internal function that isn't intended to be jumped indirectly to)
    if (mprotect(align_ptr(reinterpret_cast<void *>(loader_dlopen)), getpagesize(), PROT_READ | PROT_EXEC) != 0)
        return;

    // Passing dlopen as a caller address tricks the linker into using the internal unrestricted namespace letting us access libraries that are normally forbidden in the classloader namespace imposed on apps
    auto ldHandle{loader_dlopen("ld-android.so", RTLD_LAZY, reinterpret_cast<void *>(&dlopen))};
    if (!ldHandle)
        return;

    android_link_namespaces_all_libs = reinterpret_cast<android_link_namespaces_all_libs_t>(dlsym(ldHandle, "__loader_android_link_namespaces_all_libs"));
    if (!android_link_namespaces_all_libs)
        return;

    android_link_namespaces = reinterpret_cast<android_link_namespaces_t>(dlsym(ldHandle, "__loader_android_link_namespaces"));
    if (!android_link_namespaces)
        return;

    auto libdlAndroidHandle{loader_dlopen("libdl_android.so", RTLD_LAZY, reinterpret_cast<void *>(&dlopen))};
    if (!libdlAndroidHandle)
        return;

    loader_android_create_namespace = reinterpret_cast<loader_android_create_namespace_t>(dlsym(libdlAndroidHandle, "__loader_android_create_namespace"));
    if (!loader_android_create_namespace)
        return;

    android_get_exported_namespace = reinterpret_cast<android_get_exported_namespace_t>(dlsym(libdlAndroidHandle, "__loader_android_get_exported_namespace"));
    if (!android_get_exported_namespace)
        return;

    // Lib is now safe to use
    lib_loaded = true;
}
