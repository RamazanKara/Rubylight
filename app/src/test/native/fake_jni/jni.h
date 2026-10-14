// The part of <jni.h> that android_control.c uses, so native tests can call its JNI entry
// points without a JDK. Arrays are FakeByteArray pointers.
#pragma once
#include <stdint.h>

typedef uint8_t jboolean;
typedef int8_t jbyte;
typedef int32_t jint;
typedef int64_t jlong;
typedef float jfloat;
typedef jint jsize;
typedef void* jobject;
typedef jobject jclass;
typedef jobject jstring;
typedef jobject jarray;
typedef jarray jbyteArray;

#define JNI_FALSE 0
#define JNI_TRUE 1
#define JNIEXPORT
#define JNICALL

struct JNINativeInterface;
typedef const struct JNINativeInterface* JNIEnv;

struct JNINativeInterface {
    jsize (*GetArrayLength)(JNIEnv* env, jarray array);
    void (*GetByteArrayRegion)(JNIEnv* env, jbyteArray array, jsize start, jsize length, jbyte* buffer);
    jboolean (*ExceptionCheck)(JNIEnv* env);
};

typedef struct {
    jsize length;
    const uint8_t* bytes;
} FakeByteArray;

static jsize fakeGetArrayLength(JNIEnv* env, jarray array) {
    (void)env;
    return ((const FakeByteArray*)array)->length;
}

static void fakeGetByteArrayRegion(JNIEnv* env, jbyteArray array, jsize start, jsize length, jbyte* buffer) {
    (void)env;
    const FakeByteArray* fake = (const FakeByteArray*)array;
    for (jsize i = 0; i < length; i++) buffer[i] = (jbyte)fake->bytes[start + i];
}

static jboolean fakeExceptionCheck(JNIEnv* env) {
    (void)env;
    return JNI_FALSE;
}

static const struct JNINativeInterface fakeJniInterface = {
    fakeGetArrayLength, fakeGetByteArrayRegion, fakeExceptionCheck,
};
