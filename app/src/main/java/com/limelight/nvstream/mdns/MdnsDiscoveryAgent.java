package com.limelight.nvstream.mdns;

import com.limelight.LimeLog;

import java.net.Inet4Address;
import java.net.Inet6Address;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;

public abstract class MdnsDiscoveryAgent {
    protected MdnsDiscoveryListener listener;

    protected final HashSet<MdnsComputer> computers = new HashSet<>();
    // Hosts that answered in an earlier session. Discovery restarts on every PC list resume and
    // menu, which must not repeat their STUN and serverinfo probes; unanswered hosts are retried.
    // Each host keeps only the address of its latest answer, which is the one polling uses.
    private final HashSet<MdnsComputer> answeredComputers = new HashSet<>();
    private long discoveryGeneration;

    public MdnsDiscoveryAgent(MdnsDiscoveryListener listener) {
        this.listener = listener;
    }

    public abstract void startDiscovery(final int discoveryIntervalMs);

    public abstract void stopDiscovery();

    protected final long resetDiscovery() {
        synchronized (computers) {
            computers.clear();
            return ++discoveryGeneration;
        }
    }

    protected final long getDiscoveryGeneration() {
        synchronized (computers) {
            return discoveryGeneration;
        }
    }

    protected void reportNewComputer(String name, int port, Inet4Address[] v4Addrs, Inet6Address[] v6Addrs) {
        reportNewComputer(name, port, v4Addrs, v6Addrs, getDiscoveryGeneration());
    }

    protected void reportNewComputer(String name, int port, Inet4Address[] v4Addrs, Inet6Address[] v6Addrs,
                                     long generation) {
        LimeLog.info("mDNS: "+name+" has "+v4Addrs.length+" IPv4 addresses");
        LimeLog.info("mDNS: "+name+" has "+v6Addrs.length+" IPv6 addresses");

        Inet6Address v6GlobalAddr = getBestIpv6Address(v6Addrs);

        // Add a computer object for each IPv4 address reported by the PC
        for (Inet4Address v4Addr : v4Addrs) {
            if (!addComputer(new MdnsComputer(name, v4Addr, v6GlobalAddr, port), generation)) {
                return;
            }
        }
        // If there were no IPv4 addresses, use IPv6 for registration
        if (v4Addrs.length == 0) {
            Inet6Address v6LocalAddr = getLocalAddress(v6Addrs);

            if (v6LocalAddr != null || v6GlobalAddr != null) {
                addComputer(new MdnsComputer(name, v6LocalAddr, v6GlobalAddr, port), generation);
            }
        }
    }

    // Returns false once a newer discovery session owns the cache.
    private boolean addComputer(MdnsComputer computer, long generation) {
        boolean probe;
        synchronized (computers) {
            // Resolution can finish after stop/start. Only this session may populate its cache.
            if (generation != discoveryGeneration) return false;
            probe = computers.add(computer) && !answeredComputers.contains(computer);
        }
        // The probe can block on the network, so it runs without the cache lock.
        if (probe && listener.notifyComputerAdded(computer)) {
            synchronized (computers) {
                // Polling now uses this address. An earlier one of the same host must be probed
                // again if the host returns to it, or polling would keep trying this one.
                answeredComputers.removeIf(answered -> answered.getName().equals(computer.getName()));
                answeredComputers.add(computer);
            }
        }
        return true;
    }

    // A deleted PC must be able to return the next time discovery finds it.
    public void forgetAnsweredComputers() {
        synchronized (computers) {
            answeredComputers.clear();
        }
    }

    public List<MdnsComputer> getComputerSet() {
        synchronized (computers) {
            return new ArrayList<>(computers);
        }
    }

    protected static Inet6Address getLocalAddress(Inet6Address[] addresses) {
        for (Inet6Address addr : addresses) {
            if (addr.isLinkLocalAddress() || addr.isSiteLocalAddress()) {
                return addr;
            }
            // fc00::/7 - ULAs
            else if ((addr.getAddress()[0] & 0xfe) == 0xfc) {
                return addr;
            }
        }

        return null;
    }

    protected static Inet6Address getLinkLocalAddress(Inet6Address[] addresses) {
        for (Inet6Address addr : addresses) {
            if (addr.isLinkLocalAddress()) {
                LimeLog.info("Found link-local address: "+addr.getHostAddress());
                return addr;
            }
        }

        return null;
    }

    protected static Inet6Address getBestIpv6Address(Inet6Address[] addresses) {
        // First try to find a link local address, so we can match the interface identifier
        // with a global address (this will work for SLAAC but not DHCPv6).
        Inet6Address linkLocalAddr = getLinkLocalAddress(addresses);

        // We will try once to match a SLAAC interface suffix, then
        // pick the first matching address
        for (int tries = 0; tries < 2; tries++) {
            // We assume the addresses are already sorted in descending order
            // of preference from Bonjour.
            for (Inet6Address addr : addresses) {
                if (addr.isLinkLocalAddress() || addr.isSiteLocalAddress() || addr.isLoopbackAddress() ||
                    addr.isAnyLocalAddress() || addr.isMulticastAddress()) {
                    // Link-local, site-local, and loopback aren't global
                    LimeLog.info("Ignoring non-global address: "+addr.getHostAddress());
                    continue;
                }

                byte[] addrBytes = addr.getAddress();

                // 2002::/16
                if (addrBytes[0] == 0x20 && addrBytes[1] == 0x02) {
                    // 6to4 has horrible performance
                    LimeLog.info("Ignoring 6to4 address: "+addr.getHostAddress());
                    continue;
                }
                // 2001::/32
                else if (addrBytes[0] == 0x20 && addrBytes[1] == 0x01 && addrBytes[2] == 0x00 && addrBytes[3] == 0x00) {
                    // Teredo also has horrible performance
                    LimeLog.info("Ignoring Teredo address: "+addr.getHostAddress());
                    continue;
                }
                // fc00::/7
                else if ((addrBytes[0] & 0xfe) == 0xfc) {
                    // ULAs aren't global
                    LimeLog.info("Ignoring ULA: "+addr.getHostAddress());
                    continue;
                }

                // Compare the final 64-bit interface identifier and skip the address
                // if it doesn't match our link-local address.
                if (linkLocalAddr != null && tries == 0) {
                    boolean matched = true;

                    for (int i = 8; i < 16; i++) {
                        if (linkLocalAddr.getAddress()[i] != addr.getAddress()[i]) {
                            matched = false;
                            break;
                        }
                    }

                    if (!matched) {
                        LimeLog.info("Ignoring non-matching global address: "+addr.getHostAddress());
                        continue;
                    }
                }

                return addr;
            }
        }

        return null;
    }
}
