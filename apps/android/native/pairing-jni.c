#include <jni.h>
#include <stdlib.h>
#include <string.h>
#include "zuse_mobile.h"

JNIEXPORT jbyteArray JNICALL Java_sh_zuse_core_NativePairing_exchange(
    JNIEnv *env, jclass type, jbyteArray input) {
    (void)type;
    if (!input) return NULL;
    jsize length = (*env)->GetArrayLength(env, input);
    if (length < 0 || length > 24 * 1024 * 1024) return NULL;
    jbyte *bytes = (*env)->GetByteArrayElements(env, input, NULL);
    if (!bytes) return NULL;
    char *response = zuse_mobile_request((const uint8_t *)bytes, (size_t)length);
    (*env)->ReleaseByteArrayElements(env, input, bytes, JNI_ABORT);
    if (!response) return NULL;
    size_t size = strlen(response);
    jbyteArray result = (*env)->NewByteArray(env, (jsize)size);
    if (result) (*env)->SetByteArrayRegion(env, result, 0, (jsize)size, (const jbyte *)response);
    zuse_mobile_free(response);
    return result;
}
