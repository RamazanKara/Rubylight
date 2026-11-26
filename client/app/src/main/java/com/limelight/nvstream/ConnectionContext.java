package com.limelight.nvstream;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.FecChoice;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.utils.WiredLink;

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
    public int serverCodecModeSupport;
    // What the host can do, from this connection's serverinfo reply
    public HostCapabilities hostCaps = HostCapabilities.UNKNOWN;
    // This device's "Display on the PC" choice for this host, read once the host is known
    public HostDisplayChoice hostDisplay = HostDisplayChoice.HOST_DEFAULT;

    // This is the sessionUrl0 tag from /resume and /launch
    public String rtspSessionUrl;
    // The host's launch warning (<clientWarning>, <clientWarningMessage>), already cleaned; null for none.
    public String launchWarningCode;
    public String launchWarningMessage;
    public boolean launchActionCompleted;
    public String preparedControllers = "";
    // The video FEC to ask for; null leaves it to the host and sends nothing
    public FecChoice fecChoice;
    // Ask for mid-stream rate changes (rlRateChange=1, control message 0x553C)
    public boolean rateChange;
    // Set only for the launch that retries after the user confirmed closing the running game
    public boolean confirmReplace;
    // What an automatic start says when another game runs on the PC
    public String gameRunningMessage;
    // The bitrate the stream starts at: the configured one, lower after a link test, or PyroWave's own
    public int startBitrateKbps;
    // This device's route toward the host, looked up once at launch: PyroWave's default stays within
    // its link, and the pacing hints tell the host about it
    public WiredLink.Hints linkHints;

    public int negotiatedWidth, negotiatedHeight;
    // HDR is asked for in the launch only when this is set; 10-bit SDR negotiates 10-bit without it.
    public boolean negotiatedHdr;
    // The HdrFormatPolicy bit an HDR stream asks a Rubylight 3.0 host for; 0 for stock HDR10.
    public int hdrFormat;
    public boolean negotiated10Bit;
    public int negotiatedVideoFormats;

    public int negotiatedRemoteStreaming;
    public int negotiatedPacketSize;
    // The network it was negotiated on (Network.getNetworkHandle; 0: none).
    public long packetNetwork;
    // Jumbo packets were asked for but did not pass the pre-flight or the PC's offer.
    public boolean jumboFallback;

    public int videoCapabilities;
}
