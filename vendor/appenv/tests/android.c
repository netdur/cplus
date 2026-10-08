#include <jni.h>
#include "appenv_probe.h"

JNIEXPORT jint JNICALL
Java_dev_cplus_appenvprobe_ProbeActivity_run(JNIEnv *env, jclass cls, jobject activity) {
    (void)cls;
    JavaVM *vm = NULL;
    if ((*env)->GetJavaVM(env, &vm) != JNI_OK) return -1;
    jobject host = (*env)->NewGlobalRef(env, activity);
    if (!host) return -1;
    /* The Application context, as facet_android publishes it before the
       app's entry runs: what a lookup has before any window (S45). */
    jclass ctx_cls = (*env)->GetObjectClass(env, activity);
    jmethodID get_app = (*env)->GetMethodID(env, ctx_cls, "getApplicationContext",
                                            "()Landroid/content/Context;");
    jobject app_local = get_app ? (*env)->CallObjectMethod(env, activity, get_app) : NULL;
    jobject app = app_local ? (*env)->NewGlobalRef(env, app_local) : NULL;
    if (!app) { (*env)->DeleteGlobalRef(env, host); return -1; }
    int rc = _appenv_probe_main((uint8_t *)host, (uint8_t *)vm, (uint8_t *)app);
    (*env)->DeleteGlobalRef(env, app);
    (*env)->DeleteGlobalRef(env, host);
    return rc;
}
