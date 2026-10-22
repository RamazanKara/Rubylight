package com.limelight.nativeapi;

import android.util.Base64;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;

import java.security.cert.CertificateEncodingException;
import java.security.cert.X509Certificate;

/** What the Rust client needs to reach one paired host: where it is, who it is, and who we are. */
public final class HostEndpoint {
    public final String hostUuid;
    public final String address;
    public final int httpPort;
    /** 0 when unknown; the client then asks the host's serverinfo. */
    public final int httpsPort;
    public final String serverCertificatePem;
    public final String uniqueId;
    public final byte[] clientCertificateDer;
    public final byte[] clientPrivateKeyPkcs8;

    public HostEndpoint(String hostUuid, String address, int httpPort, int httpsPort,
                        String serverCertificatePem, String uniqueId,
                        byte[] clientCertificateDer, byte[] clientPrivateKeyPkcs8) {
        this.hostUuid = hostUuid;
        this.address = address;
        this.httpPort = httpPort;
        this.httpsPort = httpsPort;
        this.serverCertificatePem = serverCertificatePem;
        this.uniqueId = uniqueId;
        this.clientCertificateDer = clientCertificateDer;
        this.clientPrivateKeyPkcs8 = clientPrivateKeyPkcs8;
    }

    /** The endpoint of a host we are connected to, or null when it has no address or pinned certificate. */
    public static HostEndpoint of(ComputerDetails computer, String uniqueId, LimelightCryptoProvider crypto) {
        return of(computer.uuid, computer.activeAddress, computer.httpsPort, computer.serverCert, uniqueId, crypto);
    }

    /** The endpoint of a host at the address, or null when the address or the pinned certificate is missing. */
    public static HostEndpoint of(String hostUuid, ComputerDetails.AddressTuple address, int httpsPort,
                                  X509Certificate serverCert, String uniqueId, LimelightCryptoProvider crypto) {
        if (address == null || serverCert == null) {
            return null;
        }
        try {
            return new HostEndpoint(hostUuid, address.address, address.port, Math.max(httpsPort, 0),
                    pem(serverCert), uniqueId, crypto.getClientCertificate().getEncoded(),
                    crypto.getClientPrivateKey().getEncoded());
        } catch (CertificateEncodingException e) {
            return null;
        }
    }

    static String pem(X509Certificate certificate) throws CertificateEncodingException {
        String base64 = Base64.encodeToString(certificate.getEncoded(), Base64.NO_WRAP);
        StringBuilder pem = new StringBuilder("-----BEGIN CERTIFICATE-----\n");
        for (int i = 0; i < base64.length(); i += 64) {
            pem.append(base64, i, Math.min(i + 64, base64.length())).append('\n');
        }
        return pem.append("-----END CERTIFICATE-----\n").toString();
    }
}
