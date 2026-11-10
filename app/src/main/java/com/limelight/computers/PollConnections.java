package com.limelight.computers;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvHTTP;

import java.io.IOException;
import java.security.cert.X509Certificate;
import java.util.HashMap;
import java.util.Iterator;
import java.util.Objects;

/**
 * The HTTP clients of the PC list's polls, kept from one poll to the next. Every saved PC is asked
 * at each of its addresses every second or two, and a new {@link NvHTTP} reads this device's
 * certificate and key from storage and builds its trust managers and clients again each time. One
 * that is kept holds no socket and no thread, only what it was built from.
 *
 * A client serves one address and one pinned certificate, and one poll at a time: it is taken out
 * while a poll uses it, so a poll of the same address that overlaps (the last one's request still
 * running) builds its own, as every poll did before. A PC that is paired again, forgotten or reached
 * at another address is asked with another client, and the one left behind goes when it has not
 * been used for {@link #IDLE_MS}, or when the polls stop.
 */
final class PollConnections {
    /** How long an unused client is kept. A PC is polled far more often while its list shows. */
    static final long IDLE_MS = 30_000;

    /** Builds the client a poll would have built for itself. */
    interface Opener {
        NvHTTP open(ComputerDetails.AddressTuple address, int httpsPort, X509Certificate serverCert) throws IOException;
    }

    /** A client taken for one poll, which {@link #giveBack} returns. */
    static final class Lease {
        final NvHTTP http;
        private final Key key;
        private final int generation;

        private Lease(NvHTTP http, Key key, int generation) {
            this.http = http;
            this.key = key;
            this.generation = generation;
        }
    }

    /** The address and the certificate a client was built for; copied, since an address can be edited. */
    private static final class Key {
        private final String address;
        private final int port;
        private final X509Certificate serverCert;

        Key(ComputerDetails.AddressTuple address, X509Certificate serverCert) {
            this.address = address.address;
            this.port = address.port;
            this.serverCert = serverCert;
        }

        @Override
        public boolean equals(Object other) {
            if (!(other instanceof Key)) {
                return false;
            }
            Key that = (Key) other;
            // The same certificate as far as the client's own check of the host goes.
            return port == that.port && address.equals(that.address) && Objects.equals(serverCert, that.serverCert);
        }

        @Override
        public int hashCode() {
            // Without the certificate: hashing one encodes it, and an address rarely has two.
            return address.hashCode() * 31 + port;
        }
    }

    private static final class Idle {
        final NvHTTP http;
        final long sinceMs;

        Idle(NvHTTP http, long sinceMs) {
            this.http = http;
            this.sinceMs = sinceMs;
        }
    }

    private final Opener opener;
    private final HashMap<Key, Idle> idle = new HashMap<>();
    // Counts clear(): a client taken before one is not kept when its poll ends after it.
    private int generation;

    PollConnections(Opener opener) {
        this.opener = opener;
    }

    /**
     * The client for a poll of this address with this pinned certificate, starting at the HTTPS port
     * a new one would be given (0 asks the host for it), so a kept one asks exactly as a new one does.
     */
    Lease take(ComputerDetails.AddressTuple address, int httpsPort, X509Certificate serverCert) throws IOException {
        Key key = new Key(address, serverCert);
        Idle kept;
        int taken;
        synchronized (this) {
            kept = idle.remove(key);
            taken = generation;
        }
        if (kept != null) {
            kept.http.setHttpsPort(httpsPort);
            return new Lease(kept.http, key, taken);
        }
        // Outside the lock: this reads the device's certificate and key from storage.
        return new Lease(opener.open(address, httpsPort, serverCert), key, taken);
    }

    /** Keeps the client of a finished poll for the next one, and drops those no poll has used for a while. */
    synchronized void giveBack(Lease lease, long nowMs) {
        for (Iterator<Idle> kept = idle.values().iterator(); kept.hasNext(); ) {
            if (nowMs - kept.next().sinceMs >= IDLE_MS) {
                kept.remove();
            }
        }
        if (lease.generation == generation) {
            idle.put(lease.key, new Idle(lease.http, nowMs));
        }
    }

    /** Drops every kept client, and those out on a poll when they come back. */
    synchronized void clear() {
        generation++;
        idle.clear();
    }

    /** How many clients wait for their next poll. */
    synchronized int kept() {
        return idle.size();
    }
}
