# Application.mk for Moonlight

# Our minimum version is Android 5.0
APP_PLATFORM := android-21
APP_STL := c++_static

# We support 16KB pages
APP_SUPPORT_FLEXIBLE_PAGE_SIZES := true
# NDK defaults cover 64-bit targets; keep the bundled 32-bit ELFs aligned too.
APP_LDFLAGS += -Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384
