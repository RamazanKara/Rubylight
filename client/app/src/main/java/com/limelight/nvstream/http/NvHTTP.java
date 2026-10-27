package com.limelight.nvstream.http;

import android.os.Build;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.InputStream;
import java.io.InterruptedIOException;
import java.io.Reader;
import java.io.StringReader;
import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.SocketAddress;
import java.net.Proxy;
import java.net.Socket;
import java.nio.ByteBuffer;
import java.nio.channels.DatagramChannel;
import java.nio.charset.StandardCharsets;
import java.security.KeyManagementException;
import java.security.KeyStore;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.security.Principal;
import java.security.PrivateKey;
import java.security.SecureRandom;
import java.security.cert.Certificate;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import java.util.LinkedList;
import java.util.ListIterator;
import java.util.Stack;
import java.util.UUID;
import java.util.Set;
import java.util.Collections;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.IntConsumer;

import javax.net.SocketFactory;
import javax.net.ssl.HostnameVerifier;
import javax.net.ssl.HttpsURLConnection;
import javax.net.ssl.KeyManager;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLHandshakeException;
import javax.net.ssl.SSLPeerUnverifiedException;
import javax.net.ssl.SSLSession;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.SSLSocketFactory;
import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509KeyManager;
import javax.net.ssl.X509TrustManager;

import org.xmlpull.v1.XmlPullParser;
import org.xmlpull.v1.XmlPullParserException;
import org.xmlpull.v1.XmlPullParserFactory;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.nvstream.ConnectionContext;
import com.limelight.nvstream.PyroWavePace;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.utils.RedactedLog;

import okhttp3.ConnectionPool;
import okhttp3.Call;
import okhttp3.HttpUrl;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.RequestBody;
import okhttp3.MediaType;
import okhttp3.Response;
import okhttp3.ResponseBody;
import okio.BufferedSource;
import okio.ForwardingSource;
import okio.Okio;


public class NvHTTP {
    private final Set<Call> pendingCalls = Collections.newSetFromMap(new ConcurrentHashMap<>());
    private volatile boolean cancelled;
    // From the last serverinfo reply this instance read; app lists interpret control tiles by it.
    private volatile HostCapabilities hostCaps = HostCapabilities.UNKNOWN;
    private String uniqueId;
    private PairingManager pm;

    private static final int DEFAULT_HTTPS_PORT = 47984;
    public static final int DEFAULT_HTTP_PORT = 47989;
    public static final int SHORT_CONNECTION_TIMEOUT = 3000;
    public static final int LONG_CONNECTION_TIMEOUT = 5000;
    public static final int READ_TIMEOUT = 7000;
    public static final int PYROWAVE_BANDWIDTH_PROBE_BYTES = 32 * 1024 * 1024;
    static final int MAX_XML_RESPONSE_BYTES = 256 * 1024;
    static final int MAX_APP_LIST_BYTES = 4 * 1024 * 1024;

    // Print URL and content to logcat on debug builds
    private static boolean verbose = BuildConfig.DEBUG;

    private HttpUrl baseUrlHttp;

    private int httpsPort;
    
    private OkHttpClient httpClientLongConnectTimeout;
    private OkHttpClient httpClientLongConnectNoReadTimeout;
    private OkHttpClient httpClientShortConnectTimeout;

    private X509TrustManager defaultTrustManager;
    private X509TrustManager trustManager;
    private X509KeyManager keyManager;
    private X509Certificate serverCert;
    private InetAddress scopedAddress;

    void setServerCert(X509Certificate serverCert) {
        this.serverCert = serverCert;
    }

    private static X509TrustManager getDefaultTrustManager() {
        try {
            TrustManagerFactory tmf = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
            tmf.init((KeyStore) null);

            for (TrustManager tm : tmf.getTrustManagers()) {
                if (tm instanceof X509TrustManager) {
                    return (X509TrustManager) tm;
                }
            }
        } catch (NoSuchAlgorithmException e) {
            throw new RuntimeException(e);
        } catch (KeyStoreException e) {
            throw new RuntimeException(e);
        }

        throw new IllegalStateException("No X509 trust manager found");
    }

    private void initializeHttpState(final LimelightCryptoProvider cryptoProvider, final InetAddress scopedAddress) {
        this.scopedAddress = scopedAddress;
        keyManager = new X509KeyManager() {
            public String chooseClientAlias(String[] keyTypes,
                    Principal[] issuers, Socket socket) { return "Limelight-RSA"; }
            public String chooseServerAlias(String keyType, Principal[] issuers,
                    Socket socket) { return null; }
            public X509Certificate[] getCertificateChain(String alias) {
                return new X509Certificate[] {cryptoProvider.getClientCertificate()};
            }
            public String[] getClientAliases(String keyType, Principal[] issuers) { return null; }
            public PrivateKey getPrivateKey(String alias) {
                return cryptoProvider.getClientPrivateKey();
            }
            public String[] getServerAliases(String keyType, Principal[] issuers) { return null; }
        };

        defaultTrustManager = getDefaultTrustManager();
        trustManager = new X509TrustManager() {
            public X509Certificate[] getAcceptedIssuers() {
                return new X509Certificate[0];
            }
            public void checkClientTrusted(X509Certificate[] certs, String authType) {
                throw new IllegalStateException("Should never be called");
            }
            public void checkServerTrusted(X509Certificate[] certs, String authType) throws CertificateException {
                if (NvHTTP.this.serverCert != null) {
                    if (certs.length == 0 || !certs[0].equals(NvHTTP.this.serverCert)) {
                        throw new CertificateException("Certificate mismatch");
                    }
                }
                else {
                    defaultTrustManager.checkServerTrusted(certs, authType);
                }
            }
        };

        HostnameVerifier hv = new HostnameVerifier() {
            public boolean verify(String hostname, SSLSession session) {
                try {
                    Certificate[] certificates = session.getPeerCertificates();
                    if (certificates.length > 0 && certificates[0].equals(NvHTTP.this.serverCert)) {
                        // Allow any hostname if it's our pinned cert
                        return true;
                    }
                } catch (SSLPeerUnverifiedException e) {
                    e.printStackTrace();
                }

                // Fall back to default HostnameVerifier for validating CA-issued certs
                return HttpsURLConnection.getDefaultHostnameVerifier().verify(hostname, session);
            }
        };

        httpClientLongConnectTimeout = new OkHttpClient.Builder()
                .connectionPool(new ConnectionPool(0, 1, TimeUnit.MILLISECONDS))
                .hostnameVerifier(hv)
                .readTimeout(READ_TIMEOUT, TimeUnit.MILLISECONDS)
                .connectTimeout(LONG_CONNECTION_TIMEOUT, TimeUnit.MILLISECONDS)
                .proxy(Proxy.NO_PROXY)
                .fastFallback(false)
                .build();

        if (scopedAddress != null) {
            // HttpUrl rejects zone IDs, and OkHttp bypasses Dns for IP literals.
            // Restore the scope at connect time while keeping the literal HTTP host.
            httpClientLongConnectTimeout = httpClientLongConnectTimeout.newBuilder()
                    .socketFactory(new SocketFactory() {
                        @Override
                        public Socket createSocket() {
                            return new Socket() {
                                @Override
                                public void connect(SocketAddress endpoint, int timeout) throws IOException {
                                    InetSocketAddress target = (InetSocketAddress) endpoint;
                                    if (scopedAddress.equals(target.getAddress())) {
                                        target = new InetSocketAddress(scopedAddress, target.getPort());
                                    }
                                    super.connect(target, timeout);
                                }
                            };
                        }

                        @Override
                        public Socket createSocket(String host, int port) throws IOException {
                            return new Socket(host, port);
                        }

                        @Override
                        public Socket createSocket(String host, int port, InetAddress local, int localPort) throws IOException {
                            return new Socket(host, port, local, localPort);
                        }

                        @Override
                        public Socket createSocket(InetAddress host, int port) throws IOException {
                            return new Socket(host, port);
                        }

                        @Override
                        public Socket createSocket(InetAddress host, int port, InetAddress local, int localPort) throws IOException {
                            return new Socket(host, port, local, localPort);
                        }
                    }).build();
        }
        httpClientShortConnectTimeout = httpClientLongConnectTimeout.newBuilder()
                .connectTimeout(SHORT_CONNECTION_TIMEOUT, TimeUnit.MILLISECONDS)
                .build();

        httpClientLongConnectNoReadTimeout = httpClientLongConnectTimeout.newBuilder()
                .readTimeout(0, TimeUnit.MILLISECONDS)
                .build();
    }

    public HttpUrl getHttpsUrl(boolean likelyOnline) throws IOException {
        if (httpsPort == 0) {
            // Fetch the HTTPS port if we don't have it already
            httpsPort = getHttpsPort(openHttpConnectionToString(likelyOnline ? httpClientLongConnectTimeout : httpClientShortConnectTimeout,
                    baseUrlHttp, "serverinfo"));
        }

        return new HttpUrl.Builder().scheme("https").host(baseUrlHttp.host()).port(httpsPort).build();
    }
    
    public NvHTTP(ComputerDetails.AddressTuple address, int httpsPort, String uniqueId, X509Certificate serverCert, LimelightCryptoProvider cryptoProvider) throws IOException {
        RedactedLog.addSecret(address.address);
        this.uniqueId = uniqueId;

        this.serverCert = serverCert;

        this.httpsPort = httpsPort;
        InetAddress scopedAddress = null;

        try {
            // If this is an IPv4-mapped IPv6 address, OkHTTP will choke on it if it's
            // in IPv6 form, because InetAddress.getByName() will return an Inet4Address
            // for what OkHTTP thinks is an IPv6 address. Normalize it into IPv4 form
            // to avoid triggering this bug.
            String addressString = address.address;
            if (addressString.contains(":") && addressString.contains(".")) {
                InetAddress addr = InetAddress.getByName(addressString);
                if (addr instanceof Inet4Address) {
                    addressString = ((Inet4Address)addr).getHostAddress();
                }
            }

            if (addressString.contains(":") && addressString.contains("%")) {
                scopedAddress = InetAddress.getByName(addressString);
                addressString = addressString.substring(0, addressString.lastIndexOf('%'));
            }

            this.baseUrlHttp = new HttpUrl.Builder()
                    .scheme("http")
                    .host(addressString)
                    .port(address.port)
                    .build();
        } catch (IllegalArgumentException e) {
            // Encapsulate IllegalArgumentException into IOException for callers to handle more easily
            throw new IOException(e);
        }

        initializeHttpState(cryptoProvider, scopedAddress);
        this.pm = new PairingManager(this, cryptoProvider);
    }

    static String getXmlString(Reader r, String tagname, boolean throwIfMissing) throws XmlPullParserException, IOException {
        XmlPullParserFactory factory = XmlPullParserFactory.newInstance();
        factory.setNamespaceAware(true);
        XmlPullParser xpp = factory.newPullParser();

        xpp.setInput(r);
        int eventType = xpp.getEventType();
        Stack<String> currentTag = new Stack<String>();
        
        while (eventType != XmlPullParser.END_DOCUMENT) {
            switch (eventType) {
            case (XmlPullParser.START_TAG):
                if (xpp.getName().equals("root")) {
                    verifyResponseStatus(xpp);
                }
                currentTag.push(xpp.getName());
                break;
            case (XmlPullParser.END_TAG):
                currentTag.pop();
                break;
            case (XmlPullParser.TEXT):
                if (!currentTag.isEmpty() && currentTag.peek().equals(tagname)) {
                    return xpp.getText();
                }
                break;
            }
            eventType = xpp.next();
        }

        if (throwIfMissing) {
            // We throw an XmlPullParserException here for ease of handling in all the various callers.
            // We could also throw an IOException, but some callers expect those in cases where the
            // host may not be reachable. We want to distinguish unreachable hosts vs. hosts that
            // are returning garbage XML to us, so we use XmlPullParserException instead.
            throw new XmlPullParserException("Missing mandatory field in host response: "+tagname);
        }

        return null;
    }

    static String getXmlString(String str, String tagname, boolean throwIfMissing) throws XmlPullParserException, IOException {
        return getXmlString(new StringReader(str), tagname, throwIfMissing);
    }

    private static void verifyResponseStatus(XmlPullParser xpp) throws HostHttpResponseException, XmlPullParserException {
        // We use Long.parseLong() because in rare cases GFE can send back a status code of
        // 0xFFFFFFFF, which will cause Integer.parseInt() to throw a NumberFormatException due
        // to exceeding Integer.MAX_VALUE. We'll get the desired error code of -1 by just casting
        // the resulting long into an int.
        int statusCode;
        try {
            long value = Long.parseLong(xpp.getAttributeValue(XmlPullParser.NO_NAMESPACE, "status_code"));
            if (value < Integer.MIN_VALUE || value > 0xFFFFFFFFL) {
                throw new NumberFormatException();
            }
            statusCode = (int) value;
        } catch (NumberFormatException e) {
            // Garbage from the host, like a missing field (see getXmlString). Not a host error:
            // the host gave no code and no reason, so the UI must not show one as its own.
            throw new XmlPullParserException("Malformed status code in host response");
        }
        if (statusCode != 200) {
            String statusMsg = xpp.getAttributeValue(XmlPullParser.NO_NAMESPACE, "status_message");
            if (statusCode == -1 && "Invalid".equals(statusMsg)) {
                // Special case handling an audio capture error which GFE doesn't
                // provide any useful status message for.
                statusCode = 418;
                statusMsg = "Missing audio capture device. Reinstall GeForce Experience.";
            }
            throw new HostHttpResponseException(statusCode, statusMsg);
        }
    }
    
    public String getServerInfo(boolean likelyOnline) throws IOException, XmlPullParserException {
        String resp;

        // If we believe the PC is online, give it a little extra time to respond
        OkHttpClient client = likelyOnline ? httpClientLongConnectTimeout : httpClientShortConnectTimeout;
        
        //
        // TODO: Shield Hub uses HTTP for this and is able to get an accurate PairStatus with HTTP.
        // For some reason, we always see PairStatus is 0 over HTTP and only 1 over HTTPS. It looks
        // like there are extra request headers required to make this stuff work over HTTP.
        //

        // When we have a pinned cert, use HTTPS to fetch serverinfo and fall back on cert mismatch
        if (serverCert != null) {
            try {
                try {
                    resp = openHttpConnectionToString(client, getHttpsUrl(likelyOnline), "serverinfo");
                } catch (SSLHandshakeException e) {
                    // Detect if we failed due to a server cert mismatch
                    if (e.getCause() instanceof CertificateException) {
                        // Jump to the GfeHttpResponseException exception handler to retry
                        // over HTTP which will allow us to pair again to update the cert
                        throw new HostHttpResponseException(401, "Server certificate mismatch");
                    }
                    else {
                        throw e;
                    }
                }

                // This will throw an exception if the request came back with a failure status.
                // We want this because it will throw us into the HTTP case if the client is unpaired.
                getServerVersion(resp);
            }
            catch (HostHttpResponseException e) {
                if (e.getErrorCode() == 401) {
                    // Cert validation error - fall back to HTTP
                    return openHttpConnectionToString(client, baseUrlHttp, "serverinfo");
                }

                // If it's not a cert validation error, throw it
                throw e;
            }

            return resp;
        }
        else {
            // No pinned cert, so use HTTP
            return openHttpConnectionToString(client, baseUrlHttp, "serverinfo");
        }
    }

    private static ComputerDetails.AddressTuple makeTuple(String address, int port) {
        if (address == null) {
            return null;
        }

        return new ComputerDetails.AddressTuple(address, port);
    }

    public ComputerDetails getComputerDetails(String serverInfo) throws IOException, XmlPullParserException {
        ComputerDetails details = new ComputerDetails();

        details.name = getXmlString(serverInfo, "hostname", false);
        if (details.name == null || details.name.isEmpty()) {
            details.name = "UNKNOWN";
        }

        // UUID is mandatory to determine which machine is responding
        details.uuid = getXmlString(serverInfo, "uniqueid", true);
        if (details.uuid.equals(".") || details.uuid.equals("..") || details.uuid.contains("/") ||
                details.uuid.contains("\\") || details.uuid.contains(":")) {
            throw new XmlPullParserException("Invalid host uniqueid");
        }

        details.httpsPort = getHttpsPort(serverInfo);

        details.macAddress = getXmlString(serverInfo, "mac", false);

        // FIXME: Do we want to use the current port?
        details.localAddress = makeTuple(getXmlString(serverInfo, "LocalIP", false), baseUrlHttp.port());

        // This is missing on on recent GFE versions, but it's present on Sunshine
        details.externalPort = getExternalPort(serverInfo);
        details.remoteAddress = makeTuple(getXmlString(serverInfo, "ExternalIP", false), details.externalPort);

        details.pairState = getPairState(serverInfo);
        details.runningGameId = getCurrentGame(serverInfo);
        // An unknown state also produces game ID 0, but does not confirm an exit.
        details.sessionIdle = getXmlString(serverInfo, "state", true).endsWith("_SERVER_FREE");
        details.runningGameUuid = getXmlString(serverInfo, "currentgameuuid", false);
        readClientCapabilities(details, serverInfo);
        details.nvidiaServer = details.caps.family() == HostCapabilities.Family.GAMESTREAM;
        hostCaps = details.caps;

        // We could reach it so it's online
        details.state = ComputerDetails.State.ONLINE;

        return details;
    }

    static void readClientCapabilities(ComputerDetails details, String serverInfo) throws IOException, XmlPullParserException {
        details.caps = HostCapabilities.fromServerInfo(serverInfo);
        details.permission = -1;
        details.encoderBitrateKbps = details.initialEncoderBitrateKbps = 0;
        details.frameLimiterEnabled = "1".equals(getXmlString(serverInfo, "FrameLimiterEnabled", false));
        details.virtualDisplayFrameLimiterEnabled = "1".equals(getXmlString(serverInfo, "VirtualDisplayFrameLimiterEnabled", false));
        details.frameLimiterFpsLimitMilliHz = readUnsignedValue(
                getXmlString(serverInfo, "FrameLimiterFpsLimitMilliHz", false), Integer.MAX_VALUE);
        details.pyroWaveHostLinkMbps = readUnsignedValue(
                getXmlString(serverInfo, "PyroWaveHostLinkMbps", false), Integer.MAX_VALUE);
        details.pyroWaveBandwidthProbeBytes = (int) readUnsignedValue(
                getXmlString(serverInfo, "PyroWaveBandwidthProbeBytes", false), PYROWAVE_BANDWIDTH_PROBE_BYTES);
        details.pyroWaveBurstProbe = "1".equals(getXmlString(serverInfo, "PyroWaveUdpProbeVersion", false)) &&
                "1".equals(getXmlString(serverInfo, "PyroWaveUdpHandshakeVersion", false)) &&
                "1".equals(getXmlString(serverInfo, "PyroWaveUdpProbeBurstVersion", false)) &&
                "1".equals(getXmlString(serverInfo, "PyroWaveWireBudgetVersion", false));
        details.pyroWaveCriticalFecPercentage = (int) readUnsignedValue(
                getXmlString(serverInfo, "PyroWaveCriticalFecPercentage", false), 100);
        details.pyroWaveMinParityShards = (int) readUnsignedValue(
                getXmlString(serverInfo, "PyroWaveMinParityShards", false), 255);
        details.packed444Modes = readPacked444Modes(getXmlString(serverInfo, "Packed444Modes", false));

        XmlPullParser parser = XmlPullParserFactory.newInstance().newPullParser();
        parser.setInput(new StringReader(serverInfo));
        boolean encoderBitratePresent = false;
        boolean initialEncoderBitratePresent = false;
        details.serverCommands.clear();
        for (int event = parser.getEventType(); event != XmlPullParser.END_DOCUMENT; event = parser.next()) {
            if (event == XmlPullParser.START_TAG && parser.getDepth() == 2) {
                if ("Permission".equals(parser.getName())) {
                    details.permission = readUnsignedValue(parser.nextText().trim(), 0xFFFFFFFFL);
                } else if ("RustHostEncoderBitrateKbps".equals(parser.getName())) {
                    encoderBitratePresent = true;
                    details.encoderBitrateKbps = (int) readUnsignedValue(parser.nextText().trim(), 2000000);
                } else if ("RustHostInitialEncoderBitrateKbps".equals(parser.getName())) {
                    initialEncoderBitratePresent = true;
                    details.initialEncoderBitrateKbps = (int) readUnsignedValue(parser.nextText().trim(), 2000000);
                } else if ("ServerCommand".equals(parser.getName())) {
                    // Empty names still occupy a wire index.
                    details.serverCommands.add(parser.nextText());
                }
            }
        }
        details.encoderBitrateSupported = encoderBitratePresent && initialEncoderBitratePresent;
    }

    /**
     * {@code Packed444Modes} from serverinfo: the modes this client knows (bits a newer host adds
     * are left out), or -1 when absent or not a number.
     */
    static int readPacked444Modes(String value) {
        if (value == null) return -1;
        try {
            int parsed = Integer.parseInt(value.trim());
            return parsed >= 0 ? parsed & 0xF : -1;
        } catch (NumberFormatException e) {
            return -1;
        }
    }

    private static long readUnsignedValue(String value, long max) {
        try {
            long parsed = Long.parseLong(value);
            return parsed >= 0 && parsed <= max ? parsed : 0;
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    public HardwareMetrics getHardwareMetrics() throws IOException {
        // Polling must never discover a port over unauthenticated HTTP or use an unpinned host.
        if (serverCert == null || httpsPort <= 0) {
            throw new IOException("Hardware metrics require a paired host and known HTTPS port");
        }
        OkHttpClient client = httpClientShortConnectTimeout.newBuilder()
                .callTimeout(2, TimeUnit.SECONDS)
                .followRedirects(false)
                .followSslRedirects(false)
                .addInterceptor(chain -> chain.proceed(chain.request().newBuilder()
                        .header("Accept", "application/json").build()))
                .build();
        try (ResponseBody response = openHttpConnection(client, getHttpsUrl(true), "api/hardware-metrics")) {
            MediaType type = response.contentType();
            if (type == null || !"application".equals(type.type()) || !"json".equals(type.subtype())) {
                throw new IOException("Invalid hardware metrics content type");
            }
            if (response.source().request(HardwareMetrics.MAX_RESPONSE_BYTES + 1L)) {
                throw new IOException("Hardware metrics response too large");
            }
            return HardwareMetrics.parse(response.string());
        }
    }

    /** Always pinned HTTPS: the remote setup test must verify the selected PC, without HTTP fallback. */
    public ComputerDetails probePairedComputer() throws IOException, XmlPullParserException {
        if (serverCert == null || httpsPort <= 0) throw new IOException("Pair locally before testing remote access");
        OkHttpClient client = httpClientShortConnectTimeout.newBuilder().callTimeout(3, TimeUnit.SECONDS)
                .followRedirects(false).followSslRedirects(false).build();
        try (ResponseBody response = openHttpConnection(client, getHttpsUrl(true), "serverinfo")) {
            if (response.source().request(65537)) throw new IOException("Host information response too large");
            return getComputerDetails(response.string());
        }
    }

    public NetworkPathInfo getNetworkPath() throws IOException {
        if (serverCert == null || httpsPort <= 0) throw new IOException("Route diagnosis requires a paired host");
        OkHttpClient client = httpClientShortConnectTimeout.newBuilder().callTimeout(4, TimeUnit.SECONDS)
                .followRedirects(false).followSslRedirects(false).build();
        try (ResponseBody response = openHttpConnection(client, getHttpsUrl(true), "api/network-path")) {
            MediaType type = response.contentType();
            if (type == null || !"application".equals(type.type()) || !"json".equals(type.subtype()) ||
                    response.source().request(513)) throw new IOException("Invalid route response");
            return NetworkPathInfo.parse(response.string());
        }
    }
    
    public ComputerDetails getComputerDetails(boolean likelyOnline) throws IOException, XmlPullParserException {
        return getComputerDetails(getServerInfo(likelyOnline));
    }

    // This hack is Android-specific but we do it on all platforms
    // because it doesn't really matter
    private OkHttpClient performAndroidTlsHack(OkHttpClient client) {
        // Doing this each time we create a socket is required
        // to avoid the SSLv3 fallback that causes connection failures
        try {
            SSLContext sc = SSLContext.getInstance("TLS");
            sc.init(new KeyManager[] { keyManager }, new TrustManager[] { trustManager }, new SecureRandom());
            return client.newBuilder().sslSocketFactory(sc.getSocketFactory(), trustManager).build();
        } catch (NoSuchAlgorithmException | KeyManagementException e) {
            throw new RuntimeException(e);
        }
    }

    HttpUrl getCompleteUrl(HttpUrl baseUrl, String path, String query) {
        HttpUrl.Builder builder = baseUrl.newBuilder()
                .addPathSegments(path)
                .query(query)
                .addQueryParameter("uniqueid", uniqueId)
                .addQueryParameter("uuid", UUID.randomUUID().toString());
        if (path.equals("pair")) {
            // Pre-encoding the model turns spaces into literal plus signs on the host.
            builder.addQueryParameter("devicename", Build.MODEL);
        }
        return builder.build();
    }

    private ResponseBody openHttpConnection(OkHttpClient client, HttpUrl baseUrl, String path) throws IOException {
        return openHttpConnection(client, baseUrl, path, null);
    }

    // Read timeout should be enabled for any HTTP query that requires no outside action
    // on the GFE server. Examples of queries that DO require outside action are launch, resume, and quit.
    // The initial pair query does require outside action (user entering a PIN) but subsequent pairing
    // queries do not.
    private ResponseBody openHttpConnection(OkHttpClient client, HttpUrl baseUrl, String path, String query) throws IOException {
        return openHttpConnection(client, baseUrl, path, query, null);
    }

    private ResponseBody openHttpConnection(OkHttpClient client, HttpUrl baseUrl, String path, String query, RequestBody content) throws IOException {
        return openHttpConnection(client, baseUrl, path, query, content, null);
    }

    /** As above; onHeaders sees a successful response's headers before its body is read. */
    private ResponseBody openHttpConnection(OkHttpClient client, HttpUrl baseUrl, String path, String query, RequestBody content,
                                            HeadersListener onHeaders) throws IOException {
        HttpUrl completeUrl = getCompleteUrl(baseUrl, path, query);
        Request.Builder builder = new Request.Builder().url(completeUrl);
        Request request = (content == null ? builder.get() : builder.post(content)).build();
        Call call = performAndroidTlsHack(client).newCall(request);
        pendingCalls.add(call);
        if (cancelled) {
            call.cancel();
        }
        Response response;
        try {
            response = call.execute();
        } catch (IOException | RuntimeException e) {
            pendingCalls.remove(call);
            throw e;
        }

        ResponseBody body = response.body();
        
        if (response.isSuccessful()) {
            if (onHeaders != null) {
                try {
                    onHeaders.onHeaders(response.headers());
                } catch (IOException | RuntimeException e) {
                    pendingCalls.remove(call);
                    body.close();
                    throw e;
                }
            }
            // Keep cancellation effective until the caller finishes reading the body.
            BufferedSource source = Okio.buffer(new ForwardingSource(body.source()) {
                @Override
                public void close() throws IOException {
                    try {
                        super.close();
                    } finally {
                        pendingCalls.remove(call);
                    }
                }
            });
            return new ResponseBody() {
                @Override public MediaType contentType() { return body.contentType(); }
                @Override public long contentLength() { return body.contentLength(); }
                @Override public BufferedSource source() { return source; }
            };
        }

        pendingCalls.remove(call);
        
        // Unsuccessful, so close the response body
        if (body != null) {
            body.close();
        }
        
        if (response.code() == 404) {
            throw new FileNotFoundException(completeUrl.toString());
        }
        else {
            throw new HostHttpResponseException(response.code(), response.message());
        }
    }

    public void cancelPendingRequests() {
        cancelled = true;
        for (Call call : pendingCalls) {
            call.cancel();
        }
    }

    public double probePyroWaveBandwidth(ComputerDetails details, IntConsumer progress) throws IOException {
        if (serverCert == null || !details.supportsPyroWaveBandwidthProbe()) {
            throw new IOException("This paired host does not advertise the bandwidth test");
        }
        OkHttpClient client = httpClientLongConnectTimeout.newBuilder()
                .callTimeout(30, TimeUnit.SECONDS).followRedirects(false).followSslRedirects(false).build();
        long startedNs = System.nanoTime();
        try (ResponseBody response = openHttpConnection(client, getHttpsUrl(true), "pyrowave-bandwidth-probe")) {
            readBandwidthProbe(response, details.pyroWaveBandwidthProbeBytes, progress);
        }
        return details.pyroWaveBandwidthProbeBytes * 8000.0 / Math.max(1, System.nanoTime() - startedNs);
    }

    interface HeadersListener {
        void onHeaders(okhttp3.Headers headers) throws IOException;
    }

    /**
     * One frame-shaped UDP probe (Vibeshine's /pyrowave-udp-probe with handshake and burst schedule):
     * the host sends kbps for two seconds as fps video frames whose packets leave in 1 ms groups at
     * paceKbps. Returns which packets arrived, graded per frame. The host refuses while it streams.
     */
    public PyroWavePace.Result probePyroWaveUdp(int kbps, int packetSize, int fps, int paceKbps) throws IOException {
        if (serverCert == null || kbps < PyroWavePace.MIN_PROBE_KBPS || kbps > PyroWavePace.MAX_PROBE_KBPS ||
                packetSize < 256 || packetSize > 1392 || fps < 10 || fps > 500 ||
                paceKbps < kbps || paceKbps > PyroWavePace.MAX_PACE_KBPS) {
            throw new IOException("Invalid or unauthenticated PyroWave UDP probe");
        }
        InetAddress host = scopedAddress != null ? scopedAddress : InetAddress.getByName(baseUrlHttp.host());
        OkHttpClient client = httpClientLongConnectTimeout.newBuilder().readTimeout(6, TimeUnit.SECONDS)
                .callTimeout(10, TimeUnit.SECONDS).followRedirects(false).followSslRedirects(false).build();
        HttpUrl url = getHttpsUrl(true);
        return runUdpProbe(host, kbps, packetSize, fps, paceKbps, (query, onHeaders) -> {
            try (ResponseBody response = openHttpConnection(client, url, "pyrowave-udp-probe", query, null, onHeaders)) {
                return response.string();
            }
        });
    }

    interface ProbeRequest {
        /** Sends the probe request, calls onHeaders once the host announced its port, returns the body. */
        String send(String query, HeadersListener onHeaders) throws IOException;
    }

    static PyroWavePace.Result runUdpProbe(InetAddress host, int kbps, int packetSize, int fps, int paceKbps,
                                           ProbeRequest request) throws IOException {
        boolean ipv6 = host instanceof Inet6Address;
        String tokenText = UUID.randomUUID().toString().replace("-", "");
        byte[] token = tokenText.getBytes(StandardCharsets.US_ASCII);
        PyroWavePace.Result result = new PyroWavePace.Result();
        result.expected = PyroWavePace.expectedPackets(kbps, packetSize);
        boolean[] arrived = new boolean[(int) result.expected];
        AtomicLong authenticated = new AtomicLong();
        AtomicReference<Thread> receiverThread = new AtomicReference<>();
        AtomicReference<Thread> tokenThread = new AtomicReference<>();
        // DatagramChannel.socket() keeps this to API 21 calls; the channel reads into a direct buffer.
        try (DatagramChannel channel = DatagramChannel.open()) {
            channel.socket().setReceiveBufferSize(8 * 1024 * 1024);
            channel.socket().bind(new InetSocketAddress(0));
            int localPort = channel.socket().getLocalPort();
            int probeBytes = packetSize + PyroWavePace.PROBE_EXTRA_BYTES;

            HeadersListener startHandshake = headers -> {
                int udpPort = parseUdpPort(headers.get("X-PyroWave-Udp-Port"));
                if (udpPort == 0) {
                    throw new IOException("The host did not announce a valid PyroWave UDP port");
                }
                // Connected, the socket only takes the host's sending port, and reads skip address objects.
                channel.connect(new InetSocketAddress(host, udpPort));
                Thread receiver = new Thread(() -> {
                    try {
                        android.os.Process.setThreadPriority(android.os.Process.THREAD_PRIORITY_URGENT_AUDIO);
                    } catch (RuntimeException e) {
                        // Best effort; JVM tests have no Android scheduler.
                    }
                    ByteBuffer buffer = ByteBuffer.allocateDirect(2048);
                    byte[] head = new byte[36];
                    try {
                        for (;;) {
                            buffer.clear();
                            if (channel.read(buffer) != probeBytes) continue;
                            buffer.flip();
                            buffer.get(head);
                            boolean match = true;
                            for (int i = 0; i < 32 && match; i++) {
                                match = head[i] == token[i];
                            }
                            if (!match) continue;
                            authenticated.incrementAndGet();
                            long seq = ((head[32] & 0xFFL) << 24) | ((head[33] & 0xFFL) << 16) |
                                    ((head[34] & 0xFFL) << 8) | (head[35] & 0xFFL);
                            if (seq < arrived.length) {
                                arrived[(int) seq] = true;
                            }
                        }
                    } catch (IOException e) {
                        // The channel closes when the probe ends.
                    }
                }, "PyroWave probe receiver");
                receiver.start();
                receiverThread.set(receiver);
                // The host sends nothing measured until our token reaches its sending port.
                Thread sender = new Thread(() -> {
                    ByteBuffer hello = ByteBuffer.wrap(token);
                    try {
                        for (int attempt = 0; attempt < 15 && authenticated.get() == 0; attempt++) {
                            hello.rewind();
                            channel.write(hello);
                            Thread.sleep(100);
                        }
                    } catch (IOException | InterruptedException e) {
                        // A missing handshake shows as a probe without packets.
                    }
                }, "PyroWave probe handshake");
                sender.start();
                tokenThread.set(sender);
            };

            String reply = request.send("kbps=" + kbps + "&port=" + localPort + "&packetsize=" + packetSize +
                    "&token=" + tokenText + "&handshake=1&fps=" + fps + "&pacekbps=" + paceKbps, startHandshake);
            if (receiverThread.get() == null) {
                throw new IOException("The host did not start the PyroWave UDP handshake");
            }
            try {
                // A short reordered tail still counts.
                Thread.sleep(100);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                throw new InterruptedIOException();
            }
            try {
                long expected = readUnsignedValue(getXmlString(reply, "expected", true), Integer.MAX_VALUE);
                result.sent = readUnsignedValue(getXmlString(reply, "sent", true), Integer.MAX_VALUE);
                result.senderMs = Double.parseDouble(getXmlString(reply, "elapsedMs", true).trim());
                if (expected != result.expected || result.sent > expected || !Double.isFinite(result.senderMs)) {
                    throw new IOException("Invalid PyroWave UDP probe result");
                }
            } catch (XmlPullParserException | NumberFormatException e) {
                throw new IOException("Invalid PyroWave UDP probe result", e);
            }
        } finally {
            // Closing the channel ends both threads.
            joinQuietly(tokenThread.get());
            joinQuietly(receiverThread.get());
        }
        PyroWavePace.summarize(result, arrived, fps, PyroWavePace.burstGroupPackets(paceKbps, packetSize, ipv6));
        return result;
    }

    /** The host's announced UDP sending port, or 0 when the header is missing or invalid. */
    static int parseUdpPort(String header) {
        if (header == null || !header.matches("[0-9]{4,5}")) {
            return 0;
        }
        int port = Integer.parseInt(header);
        return port >= 1024 && port <= 65535 ? port : 0;
    }

    private static void joinQuietly(Thread thread) {
        if (thread == null) return;
        thread.interrupt();
        try {
            thread.join(1000);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    public double probeServerLatency() throws IOException, XmlPullParserException {
        OkHttpClient client = httpClientShortConnectTimeout.newBuilder()
                .callTimeout(2, TimeUnit.SECONDS).followRedirects(false).followSslRedirects(false).build();
        long startedNs = System.nanoTime();
        getServerVersion(openHttpConnectionToString(client, baseUrlHttp, "serverinfo"));
        return (System.nanoTime() - startedNs) / 1000000.0;
    }

    static void readBandwidthProbe(ResponseBody response, int expectedBytes, IntConsumer progress) throws IOException {
        if (expectedBytes <= 0 || expectedBytes > PYROWAVE_BANDWIDTH_PROBE_BYTES ||
                (response.contentLength() != -1 && response.contentLength() != expectedBytes)) {
            throw new IOException("Unexpected bandwidth test size");
        }
        MediaType type = response.contentType();
        if (type == null || !"application".equals(type.type()) || !"octet-stream".equals(type.subtype())) {
            throw new IOException("Unexpected bandwidth test content type");
        }
        InputStream input = response.byteStream();
        byte[] buffer = new byte[64 * 1024];
        int total = 0;
        int previousPercent = -1;
        int count;
        while ((count = input.read(buffer, 0, Math.min(buffer.length, expectedBytes - total + 1))) != -1) {
            total += count;
            if (total > expectedBytes) {
                throw new IOException("Bandwidth test exceeded advertised size");
            }
            int percent = (int) (total * 100L / expectedBytes);
            if (percent != previousPercent) {
                progress.accept(percent);
                previousPercent = percent;
            }
        }
        if (total != expectedBytes) {
            throw new IOException("Bandwidth test ended before advertised size");
        }
    }

    private String openHttpConnectionToString(OkHttpClient client, HttpUrl baseUrl, String path) throws IOException {
        return openHttpConnectionToString(client, baseUrl, path, null);
    }

    static String readXmlResponse(ResponseBody response, String path) throws IOException {
        int limit = "applist".equals(path) ? MAX_APP_LIST_BYTES : MAX_XML_RESPONSE_BYTES;
        // Content-Length may be absent, incorrect, or the size before decompression.
        // Bound the actual decoded body before ResponseBody.string() allocates it.
        if (response.contentLength() > limit || response.source().request(limit + 1L)) {
            throw new IOException("Host XML response exceeds the " + limit + " byte limit");
        }
        return response.string();
    }

    private String openHttpConnectionToString(OkHttpClient client, HttpUrl baseUrl, String path, String query) throws IOException {
        try {
            String respString;
            try (ResponseBody resp = openHttpConnection(client, baseUrl, path, query)) {
                respString = readXmlResponse(resp, path);
            }

            if (verbose && !path.equals("serverinfo") && !path.equals("pair")) {
                LimeLog.info(getCompleteUrl(baseUrl, path, query)+" -> "+respString);
            }

            return respString;
        } catch (IOException e) {
            if (verbose && !path.equals("serverinfo") && !path.equals("pair")) {
                LimeLog.warning("Host request failed: " + path + " (" + e.getClass().getSimpleName() + ")");
                e.printStackTrace();
            }
            
            throw e;
        }
    }

    public String getServerVersion(String serverInfo) throws XmlPullParserException, IOException {
        // appversion is present in all supported GFE versions
        return getXmlString(serverInfo, "appversion", true);
    }

    public PairingManager.PairState getPairState() throws IOException, XmlPullParserException {
        return getPairState(getServerInfo(true));
    }

    public PairingManager.PairState getPairState(String serverInfo) throws IOException, XmlPullParserException {
        // appversion is present in all supported GFE versions
        return NvHTTP.getXmlString(serverInfo, "PairStatus", true).equals("1") ?
                PairState.PAIRED : PairState.NOT_PAIRED;
    }
    
    public long getMaxLumaPixelsH264(String serverInfo) throws XmlPullParserException, IOException {
        // MaxLumaPixelsH264 wasn't present on old GFE versions
        String str = getXmlString(serverInfo, "MaxLumaPixelsH264", false);
        return readUnsignedValue(str, Long.MAX_VALUE);
    }
    
    public long getMaxLumaPixelsHEVC(String serverInfo) throws XmlPullParserException, IOException {
        // MaxLumaPixelsHEVC wasn't present on old GFE versions
        String str = getXmlString(serverInfo, "MaxLumaPixelsHEVC", false);
        return readUnsignedValue(str, Long.MAX_VALUE);
    }

    // Possible meaning of bits
    // Bit 0: H.264 Baseline
    // Bit 1: H.264 High
    // ----
    // Bit 8: HEVC Main
    // Bit 9: HEVC Main10
    // Bit 10: HEVC Main10 4:4:4
    // Bit 11: ???
    public long getServerCodecModeSupport(String serverInfo) throws XmlPullParserException, IOException {
        // ServerCodecModeSupport wasn't present on old GFE versions
        String str = getXmlString(serverInfo, "ServerCodecModeSupport", false);
        return readUnsignedValue(str, 0xFFFFFFFFL);
    }
    
    public String getGpuType(String serverInfo) throws XmlPullParserException, IOException {
        // ServerCodecModeSupport wasn't present on old GFE versions
        return getXmlString(serverInfo, "gputype", false);
    }

    public String getGfeVersion(String serverInfo) throws XmlPullParserException, IOException {
        // ServerCodecModeSupport wasn't present on old GFE versions
        return getXmlString(serverInfo, "GfeVersion", false);
    }
    
    public boolean supports4K(String serverInfo) throws XmlPullParserException, IOException {
        // Only allow 4K on GFE 3.x. GfeVersion wasn't present on very old versions of GFE.
        String gfeVersionStr = getXmlString(serverInfo, "GfeVersion", false);
        if (gfeVersionStr == null || gfeVersionStr.startsWith("2.")) {
            return false;
        }

        return true;
    }

    public int getCurrentGame(String serverInfo) throws IOException, XmlPullParserException {
        // GFE 2.8 started keeping currentgame set to the last game played. As a result, it no longer
        // has the semantics that its name would indicate. To contain the effects of this change as much
        // as possible, we'll force the current game to zero if the server isn't in a streaming session.
        if (getXmlString(serverInfo, "state", true).endsWith("_SERVER_BUSY")) {
            try {
                return Integer.parseInt(getXmlString(serverInfo, "currentgame", true));
            } catch (NumberFormatException e) {
                throw new XmlPullParserException("Malformed current game ID", null, e);
            }
        }
        else {
            return 0;
        }
    }

    public int getHttpsPort(String serverInfo) {
        try {
            int port = Integer.parseInt(getXmlString(serverInfo, "HttpsPort", true));
            return port > 0 && port <= 65535 ? port : DEFAULT_HTTPS_PORT;
        } catch (XmlPullParserException | IOException | NumberFormatException e) {
            e.printStackTrace();
            return DEFAULT_HTTPS_PORT;
        }
    }

    public int getExternalPort(String serverInfo) {
        // This is an extension which is not present in GFE. It is present for Sunshine to be able
        // to support dynamic HTTP WAN ports without requiring the user to manually enter the port.
        try {
            int port = Integer.parseInt(getXmlString(serverInfo, "ExternalPort", true));
            return port > 0 && port <= 65535 ? port : baseUrlHttp.port();
        } catch (XmlPullParserException e) {
            // Expected on non-Sunshine servers
            return baseUrlHttp.port();
        } catch (IOException | NumberFormatException e) {
            e.printStackTrace();
            return baseUrlHttp.port();
        }
    }

    /**
     * Get an app by ID
     * @param appId The ID of the app
     * @see #getAppByName(String) for alternative.
     * @return app details, or null if no app with that ID exists
     */
    public NvApp getAppById(int appId) throws IOException, XmlPullParserException {
        LinkedList<NvApp> appList = getAppList();
        for (NvApp appFromList : appList) {
            if (appFromList.getAppId() == appId) {
                return appFromList;
            }
        }
        return null;
    }

    public NvApp getAppByUuid(String appUuid) throws IOException, XmlPullParserException {
        for (NvApp app : getAppList()) {
            if (appUuid.equals(app.getAppUuid())) {
                return app;
            }
        }
        return null;
    }

    /**
     * Get an app by name
     * NOTE: It is perfectly valid for multiple apps to have the same name,
     * this function will only return the first one it finds.
     * Consider using getAppById instead.
     * @param appName The name of the app
     * @see #getAppById(int) for alternative.
     * @return app details, or null if no app with that name exists
     */
    public NvApp getAppByName(String appName) throws IOException, XmlPullParserException {
        LinkedList<NvApp> appList = getAppList();
        for (NvApp appFromList : appList) {
            if (appFromList.getAppName().equalsIgnoreCase(appName)) {
                return appFromList;
            }
        }
        return null;
    }

    public PairingManager getPairingManager() {
        return pm;
    }
    
    public static LinkedList<NvApp> getAppListByReader(Reader r) throws XmlPullParserException, IOException {
        return getAppListByReader(r, HostCapabilities.UNKNOWN);
    }

    /** The host's app list; reserved identities are control tiles or special apps only on hosts that give them that meaning. */
    public static LinkedList<NvApp> getAppListByReader(Reader r, HostCapabilities caps) throws XmlPullParserException, IOException {
        XmlPullParserFactory factory = XmlPullParserFactory.newInstance();
        factory.setNamespaceAware(true);
        XmlPullParser xpp = factory.newPullParser();

        xpp.setInput(r);
        int eventType = xpp.getEventType();
        LinkedList<NvApp> appList = new LinkedList<NvApp>();
        Stack<String> currentTag = new Stack<String>();
        boolean rootTerminated = false;

        while (eventType != XmlPullParser.END_DOCUMENT) {
            switch (eventType) {
            case (XmlPullParser.START_TAG):
                if (xpp.getName().equals("root")) {
                    verifyResponseStatus(xpp);
                }
                currentTag.push(xpp.getName());
                if (xpp.getName().equals("App")) {
                    NvApp added = new NvApp();
                    added.setHostCapabilities(caps);
                    appList.addLast(added);
                }
                break;
            case (XmlPullParser.END_TAG):
                currentTag.pop();
                if (xpp.getName().equals("root")) {
                    rootTerminated = true;
                }
                break;
            case (XmlPullParser.TEXT):
                if (!currentTag.contains("App")) {
                    break;
                }
                NvApp app = appList.getLast();
                if (currentTag.peek().equals("AppTitle")) {
                    app.setAppName(xpp.getText());
                } else if (currentTag.peek().equals("ID")) {
                    app.setAppId(xpp.getText());
                } else if (currentTag.peek().equals("IsHdrSupported")) {
                    app.setHdrSupported(xpp.getText().equals("1"));
                } else if (currentTag.peek().equals("UUID")) {
                    app.setAppUuid(xpp.getText());
                } else if (currentTag.peek().equals("IDX")) {
                    app.setHostIndex(xpp.getText());
                } else if (currentTag.peek().equals("ArtVersion")) {
                    app.setArtVersion(xpp.getText());
                }
                break;
            }
            eventType = xpp.next();
        }
        
        // Throw a malformed XML exception if we've not seen the root tag ended
        if (!rootTerminated) {
            throw new XmlPullParserException("Malformed XML: Root tag was not terminated");
        }
        
        // Ensure that all apps in the list are initialized
        ListIterator<NvApp> i = appList.listIterator();
        while (i.hasNext()) {
            NvApp app = i.next();
            
            // Remove uninitialized apps
            if (!app.isInitialized()) {
                LimeLog.warning("GFE returned incomplete app: "+app.getAppId()+" "+app.getAppName());
                i.remove();
            }
        }
        
        boolean monitorCatalogue = false;
        for (NvApp app : appList) {
            monitorCatalogue |= app.getControl() == NvApp.Control.DISCONNECT_MONITOR;
        }
        for (NvApp app : appList) {
            app.setMonitorResume(monitorCatalogue);
        }
        return appList;
    }
    
    public String getAppListRaw() throws IOException {
        return openHttpConnectionToString(httpClientLongConnectTimeout, getHttpsUrl(true), "applist");
    }
    
    public LinkedList<NvApp> getAppList() throws HostHttpResponseException, IOException, XmlPullParserException {
        if (verbose) {
            // Use the raw function so the app list is printed
            return getAppListByReader(new StringReader(getAppListRaw()), hostCaps);
        }
        else {
            try (final ResponseBody resp = openHttpConnection(httpClientLongConnectTimeout, getHttpsUrl(true), "applist")) {
                return getAppListByReader(new StringReader(readXmlResponse(resp, "applist")), hostCaps);
            }
        }
    }

    String executePairingCommand(String additionalArguments, boolean enableReadTimeout) throws HostHttpResponseException, IOException {
        return openHttpConnectionToString(enableReadTimeout ? httpClientLongConnectTimeout : httpClientLongConnectNoReadTimeout,
                baseUrlHttp, "pair", "updateState=1&" + additionalArguments);
    }

    String executePairingChallenge() throws HostHttpResponseException, IOException {
        return openHttpConnectionToString(httpClientLongConnectTimeout, getHttpsUrl(true),
                "pair", "updateState=1&phrase=pairchallenge");
    }

    public void unpair() throws IOException {
        openHttpConnectionToString(httpClientLongConnectTimeout, serverCert != null ? getHttpsUrl(true) : baseUrlHttp, "unpair");
    }

    void cancelPairing() throws IOException {
        // Pinning the server certificate precedes registration of our client certificate.
        openHttpConnectionToString(httpClientLongConnectTimeout, baseUrlHttp, "unpair");
    }
    
    public InputStream getBoxArt(NvApp app) throws IOException {
        ResponseBody resp = openHttpConnection(httpClientLongConnectTimeout, getHttpsUrl(true), "appasset", "appid=" + app.getAppId() + "&AssetType=2&AssetIdx=0");
        return resp.byteStream();
    }
    
    public int getServerMajorVersion(String serverInfo) throws XmlPullParserException, IOException {
        return getServerAppVersionQuad(serverInfo)[0];
    }
    
    public int[] getServerAppVersionQuad(String serverInfo) throws XmlPullParserException, IOException {
        String serverVersion = getServerVersion(serverInfo);
        if (serverVersion == null) {
            throw new XmlPullParserException("Missing server version field");
        }
        String[] serverVersionSplit = serverVersion.split("\\.");
        if (serverVersionSplit.length != 4) {
            throw new XmlPullParserException("Malformed server version field: "+serverVersion);
        }
        int[] ret = new int[serverVersionSplit.length];
        for (int i = 0; i < ret.length; i++) {
            try {
                ret[i] = Integer.parseInt(serverVersionSplit[i]);
                // Sunshine-family hosts use -1 as the trailing build number.
                if (ret[i] < 0 && !(i == 3 && ret[i] == -1)) {
                    throw new NumberFormatException();
                }
            } catch (NumberFormatException e) {
                throw new XmlPullParserException("Malformed server version field: " + serverVersion, null, e);
            }
        }
        return ret;
    }

    final private static char[] hexArray = "0123456789ABCDEF".toCharArray();
    private static String bytesToHex(byte[] bytes) {
        char[] hexChars = new char[bytes.length * 2];
        for ( int j = 0; j < bytes.length; j++ ) {
            int v = bytes[j] & 0xFF;
            hexChars[j * 2] = hexArray[v >>> 4];
            hexChars[j * 2 + 1] = hexArray[v & 0x0F];
        }
        return new String(hexChars);
    }
    
    /** The PC display choice a launch carries: only hosts with display modes receive one. */
    public static HostDisplayChoice launchDisplayChoice(ConnectionContext context, NvApp.Role role) {
        return context.hostDisplay == null || !context.hostCaps.has(HostCapabilities.Feature.HOST_DISPLAY_MODES) ?
                HostDisplayChoice.HOST_DEFAULT : context.hostDisplay.forRole(role);
    }

    /**
     * Whether this stream asks for a virtual display, whether or not the host has one: the PC
     * display choice where the host has display modes, otherwise the on/off setting.
     */
    public static boolean wantsVirtualDisplay(ConnectionContext context, NvApp.Role role) {
        return role != NvApp.Role.INPUT_ONLY &&
                launchDisplayChoice(context, role).requestsVirtualDisplay(context.streamConfig.getVirtualDisplay());
    }

    private static int fractionalLaunchRateX100(ConnectionContext context, int launchFps) {
        if (!context.hostCaps.has(HostCapabilities.Feature.FRACTIONAL_REFRESH)) return 0;
        int explicit = context.streamConfig.getLaunchRefreshRateX100();
        if (explicit >= 100 && explicit <= 100000) return explicit;
        int display = context.streamConfig.getClientRefreshRateX100();
        return display >= 100 && display <= 100000 && (display + 50L) / 100 == launchFps ? display : 0;
    }

    /** Exact client cadence, without overriding a deliberately lower capped-FPS stream. */
    public static int getStreamFrameRateMilliHz(ConnectionContext context) {
        int streamFps = context.streamConfig.getRefreshRate();
        int fractional = fractionalLaunchRateX100(context, context.streamConfig.getLaunchRefreshRate());
        return fractional > 0 && (fractional + 50L) / 100 == streamFps ?
                fractional * 10 : streamFps * 1000;
    }

    /** The launch and resume query, with each extension only for a host that shows it. */
    static String getLaunchQuery(ConnectionContext context, int appId, boolean enableHdr) {
        HostCapabilities caps = context.hostCaps;
        NvApp.Role role = context.streamConfig.getApp().getRole();
        boolean inputOnly = role == NvApp.Role.INPUT_ONLY;
        // 10-bit SDR says hdrMode=0 outright: hosts that infer it from an absent value would read a
        // plain resume as HDR. A stream that ended up 8-bit still sends nothing, as before.
        boolean tenBitSdr = context.streamConfig.isSdr10Bit() && context.negotiated10Bit;
        int gamepads = role == NvApp.Role.REMOTE_MONITOR ? 0 : context.streamConfig.getAttachedGamepadMask();
        // Using an FPS value over 60 causes SOPS to default to 720p60,
        // so force it to 0 to ensure the correct resolution is set. We
        // used to use 60 here but that locked the frame rate to 60 FPS
        // on GFE 3.20.3.
        int fps = context.isNvidiaServerSoftware && context.streamConfig.getLaunchRefreshRate() > 60 ?
                0 : context.streamConfig.getLaunchRefreshRate();
        String launchRate = Integer.toString(fps);
        // Sunshine reads the rate as an integer and would truncate 59.94 to 59.
        int fractionalRateX100 = fractionalLaunchRateX100(context, fps);
        if (fractionalRateX100 > 0) {
            launchRate = String.format(java.util.Locale.ROOT, "%.2f", fractionalRateX100 / 100.0);
        }

        HostDisplayChoice hostDisplay = launchDisplayChoice(context, role);
        boolean virtualDisplay = caps.has(HostCapabilities.Feature.VIRTUAL_DISPLAY) && wantsVirtualDisplay(context, role);
        // Vibepollo reads clientVrrRequested and switches its virtual display for it; older
        // Rubylight versions read only vrr.
        String vrr = inputOnly || !context.streamConfig.getVrr() || !caps.has(HostCapabilities.Feature.HOST_VRR) ? "" :
                caps.family() == HostCapabilities.Family.RUBYLIGHT ? "&clientVrrRequested=1&vrr=1" : "&clientVrrRequested=1";

        boolean enableSops = context.streamConfig.getSops();
        if (context.isNvidiaServerSoftware) {
            // Using an unsupported resolution (not 720p, 1080p, or 4K) causes
            // GFE to force SOPS to 720p60. This is fine for < 720p resolutions like
            // 360p or 480p, but it is not ideal for 1440p and other resolutions.
            // When we detect an unsupported resolution, disable SOPS unless it's under 720p.
            // FIXME: Detect support resolutions using the serverinfo response, not a hardcoded list
            if (context.negotiatedWidth * context.negotiatedHeight > 1280 * 720 &&
                    context.negotiatedWidth * context.negotiatedHeight != 1920 * 1080 &&
                    context.negotiatedWidth * context.negotiatedHeight != 3840 * 2160) {
                LimeLog.info("Disabling SOPS due to non-standard resolution: "+context.negotiatedWidth+"x"+context.negotiatedHeight);
                enableSops = false;
            }
        }

        return "appid=" + appId +
            (context.streamConfig.getApp().getAppUuid().isEmpty() ? "" :
                    "&" + new HttpUrl.Builder().scheme("https").host("host")
                            .addQueryParameter("appuuid", context.streamConfig.getApp().getAppUuid())
                            .build().encodedQuery()) +
            (role == NvApp.Role.REMOTE_MONITOR ? "&remote_monitor=1" :
                    inputOnly && !context.streamConfig.getApp().isSmallStreamInputOnly() ? "&input_only=1" : "") +
            "&mode=" + context.negotiatedWidth + "x" + context.negotiatedHeight + "x" + launchRate +
            (virtualDisplay ? "&virtualDisplay=1&scaleFactor=" + context.streamConfig.getVirtualDisplayScale() : "") +
            (hostDisplay.wireValue == null ? "" :
                    "&" + HostDisplayChoice.LAUNCH_PARAMETER + "=" + hostDisplay.wireValue) +
            vrr +
            "&additionalStates=1&sops=" + (enableSops ? 1 : 0) +
            "&rikey="+bytesToHex(context.riKey.getEncoded()) +
            "&rikeyid="+context.riKeyId +
            (inputOnly ? "" : !enableHdr ? (tenBitSdr ? "&hdrMode=0" : "") : "&hdrMode=1&clientHdrCapVersion=0&clientHdrCapSupportedFlagsInUint32=0&clientHdrCapMetaDataId=NV_STATIC_METADATA_TYPE_1&clientHdrCapDisplayData=0x0x0x0x0x0x0x0x0x0x0") +
            "&localAudioPlayMode=" + (context.streamConfig.getPlayLocalAudio() ? 1 : 0) +
            "&surroundAudioInfo=" + context.streamConfig.getAudioConfiguration().getSurroundAudioInfo() +
            (context.preparedControllers.isEmpty() ? "" : "&rlControllers=" + context.preparedControllers) +
            "&remoteControllersBitmap=" + gamepads +
            "&gcmap=" + gamepads +
            "&gcpersist="+(role != NvApp.Role.REMOTE_MONITOR && context.streamConfig.getPersistGamepadsAfterDisconnect() ? 1 : 0) +
            (context.fecChoice == null || !caps.has(HostCapabilities.Feature.CLIENT_FEC) ? "" :
                    "&" + FecChoice.LAUNCH_PARAMETER + "=" + context.fecChoice.wireValue);
    }

    public boolean launchApp(ConnectionContext context, String verb, int appId, boolean enableHdr) throws IOException, XmlPullParserException {
        String xmlStr = openHttpConnectionToString(httpClientLongConnectNoReadTimeout, getHttpsUrl(true), verb,
                getLaunchQuery(context, appId, enableHdr) + MoonBridge.getLaunchUrlQueryParameters());
        return readLaunchResponse(context, verb, xmlStr);
    }

    static boolean readLaunchResponse(ConnectionContext context, String verb, String xmlStr) throws IOException, XmlPullParserException {
        if ((verb.equals("launch") && !getXmlString(xmlStr, "gamesession", true).equals("0") ||
                (verb.equals("resume") && !getXmlString(xmlStr, "resume", true).equals("0")))) {
            // sessionUrl0 will be missing for older GFE versions
            context.rtspSessionUrl = getXmlString(xmlStr, "sessionUrl0", false);
            return true;
        }
        else {
            return false;
        }
    }

    public boolean disconnectRole(NvApp.Role role) throws IOException, XmlPullParserException {
        if (role == NvApp.Role.STREAM) {
            throw new IllegalArgumentException("Not a remote role");
        }
        NvApp action = new NvApp("", role == NvApp.Role.REMOTE_MONITOR ? 2147483502 : 2147483503, false);
        // Vibepollo answers a launch without a stream key or audio mode with 400 before it reads
        // the tile (nvhttp.cpp launch). This one starts no stream, so a throwaway key does.
        byte[] key = new byte[16];
        new SecureRandom().nextBytes(key);
        try {
            getXmlString(openHttpConnectionToString(httpClientLongConnectTimeout, getHttpsUrl(true),
                    "launch", "appid=" + action.getAppId() + "&rikey=" + bytesToHex(key) +
                            "&rikeyid=0&localAudioPlayMode=0"), "gamesession", true);
        } catch (HostHttpResponseException e) {
            if (LaunchConfirmation.isCompleted(action, e)) {
                return true;
            }
            throw e;
        }
        return false;
    }
    
    public boolean quitApp() throws IOException, XmlPullParserException {
        String xmlStr = openHttpConnectionToString(httpClientLongConnectNoReadTimeout, getHttpsUrl(true), "cancel");
        if (getXmlString(xmlStr, "cancel", true).equals("0")) {
            return false;
        }

        // Newer GFE versions will just return success even if quitting fails
        // if we're not the original requestor.
        if (getCurrentGame(getServerInfo(true)) != 0) {
            // Generate a synthetic GfeResponseException letting the caller know
            // that they can't kill someone else's stream.
            throw new HostHttpResponseException(599, "");
        }

        return true;
    }

    public String getClipboard() throws IOException {
        try (ResponseBody response = openHttpConnection(httpClientLongConnectTimeout, getHttpsUrl(true),
                "actions/clipboard", "type=text")) {
            return readClipboardResponse(response);
        }
    }

    public int setBitrate(int kbps) throws IOException, XmlPullParserException {
        if (kbps <= 0 || kbps > StreamConfiguration.MAX_BITRATE_KBPS) {
            throw new IllegalArgumentException("Bitrate must be between 1 and " + StreamConfiguration.MAX_BITRATE_KBPS + " kbps");
        }
        String response = openHttpConnectionToString(httpClientLongConnectTimeout, getHttpsUrl(true),
                "bitrate", "bitrate=" + kbps);
        long applied = readUnsignedValue(getXmlString(response, "bitrate", true), StreamConfiguration.MAX_BITRATE_KBPS);
        if (applied == 0) {
            throw new IOException("Host did not apply the bitrate");
        }
        return (int) applied;
    }

    static String readClipboardResponse(ResponseBody response) throws IOException {
        MediaType type = response.contentType();
        // Some Sunshine versions return an XML error with HTTP 200 for unknown endpoints.
        if (type == null || !type.type().equals("text") || !type.subtype().equals("plain")) {
            throw new IOException("Host does not support text clipboard transfer");
        }
        if (response.source().request(1024 * 1024 + 1)) {
            throw new IOException("Clipboard text exceeds 1 MiB");
        }
        return response.string();
    }

    public void sendClipboard(String text) throws IOException {
        RequestBody body = clipboardRequestBody(text);
        try (ResponseBody response = openHttpConnection(httpClientLongConnectTimeout, getHttpsUrl(true),
                "actions/clipboard", "type=text", body)) {
            if (response.byteStream().read() != -1) {
                throw new IOException("Host did not accept the clipboard text");
            }
        }
    }

    static RequestBody clipboardRequestBody(String text) throws IOException {
        byte[] bytes = text.getBytes(StandardCharsets.UTF_8);
        if (bytes.length > 1024 * 1024) {
            throw new IOException("Clipboard text exceeds 1 MiB");
        }
        return RequestBody.create(bytes, MediaType.get("text/plain; charset=utf-8"));
    }
}
