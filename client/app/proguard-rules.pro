# Constructed by EvdevCaptureProviderShim only in the root flavor.
-keep,allowoptimization class com.limelight.binding.input.evdev.EvdevCaptureProvider {
    public <init>(android.app.Activity, com.limelight.binding.input.evdev.EvdevListener);
}

# Looked up by name from callbacks.c, pyrowave_renderer.cpp and protocol/android/src/bridge.rs.
-keepclassmembers class com.limelight.nvstream.jni.MoonBridge {
    public static *** bridge*(...);
}
-keepclassmembers class com.limelight.binding.video.FrameLatencyStats {
    void onDecoderOutput(int, long, long);
    void onPresentQueued(int, long, boolean, boolean);
    void onFrameRendered(long, long);
    void discard(long, java.lang.String);
}

# The default Android rules retain native method names and descriptor classes.
# BouncyCastle discovers mappings and JCA services by name. AndroidCryptoProvider
# uses only RSA keys, SHA256withRSA signatures and X.509 certificates from BC.
-keep,allowoptimization class org.bouncycastle.jcajce.provider.asymmetric.RSA$Mappings {
    public <init>();
}
-keep,allowoptimization class org.bouncycastle.jcajce.provider.asymmetric.X509$Mappings {
    public <init>();
}
-keep,allowoptimization class org.bouncycastle.jcajce.provider.digest.SHA256$Mappings {
    public <init>();
}
-keep,allowoptimization class org.bouncycastle.jcajce.provider.asymmetric.rsa.KeyPairGeneratorSpi {
    public <init>();
}
-keep,allowoptimization class org.bouncycastle.jcajce.provider.asymmetric.rsa.KeyFactorySpi {
    public <init>();
}
-keep,allowoptimization class org.bouncycastle.jcajce.provider.asymmetric.rsa.DigestSignatureSpi$SHA256 {
    public <init>();
}
-keep,allowoptimization class org.bouncycastle.jcajce.provider.asymmetric.x509.CertificateFactory {
    public <init>();
}
-keep,allowoptimization class org.bouncycastle.jcajce.provider.digest.SHA256$Digest {
    public <init>();
}

# BC's unused LDAP certificate stores reference desktop JNDI APIs.
-dontwarn javax.naming.**

# OkHttp/Okio ship consumer rules. jcodec's H.264 parsing and
# ShieldControllerExtensions' Binder calls are statically referenced.
