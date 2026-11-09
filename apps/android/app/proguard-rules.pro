# Kotlin serialization generates and retains serializers through its consumer rules.

# JNI entry point must retain its exact JVM name.
-keep class sh.zuse.core.NativePairing { *; }
