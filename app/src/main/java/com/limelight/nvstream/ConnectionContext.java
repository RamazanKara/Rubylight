package com.limelight.nvstream;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.preferences.HostDisplayChoice;

import java.security.cert.X509Certificate;

import javax.crypto.SecretKey;

public class ConnectionContext {
    public ComputerDetails.AddressTuple serverAddress;
    public int httpsPort;
    public boolean isNvidiaServerSoftware;
    public X509Certificate serverCert;
    public StreamConfiguration streamConfig;
    public NvConnectionListener connListener;
    public SecretKey riKey;
    public int riKeyId;
    
    // This is the version quad from the appversion tag of /serverinfo
    public String serverAppVersion;
    public String serverGfeVersion;
    public int serverCodecModeSupport;
    // What the host can do, from this connection's serverinfo reply
    public HostCapabilities hostCaps = HostCapabilities.UNKNOWN;
    // This device's "Display on the PC" choice for this host, read once the host is known
    public HostDisplayChoice hostDisplay = HostDisplayChoice.HOST_DEFAULT;

    // This is the sessionUrl0 tag from /resume and /launch
    public String rtspSessionUrl;
    public boolean launchActionCompleted;
    public String preparedControllers = "";
    
    public int negotiatedWidth, negotiatedHeight;
    // HDR is asked for in the launch only when this is set; 10-bit SDR negotiates 10-bit without it.
    public boolean negotiatedHdr;
    public boolean negotiated10Bit;
    public int negotiatedVideoFormats;

    public int negotiatedRemoteStreaming;
    public int negotiatedPacketSize;

    public int videoCapabilities;
}
