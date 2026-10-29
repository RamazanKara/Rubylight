package com.limelight.utils;

import java.net.InetAddress;
import java.net.UnknownHostException;

import okhttp3.HttpUrl;

/**
 * Classifying a PC's address as local or remote is a local operation: only an IP literal is
 * classified, and a name is never looked up (a blocking DNS query each time, for an answer that
 * the connection resolves anyway). Ported from Moonlight V+ (NetHelper.isIpLiteral).
 */
public final class LiteralAddress {
    private LiteralAddress() {}

    /**
     * The address {@code host} spells out: an IPv4 dotted quad, or IPv6 with or without brackets.
     * Null for a name or anything that is not a valid literal.
     */
    public static InetAddress parse(String host) {
        if (host == null) return null;
        boolean bracketed = host.startsWith("[");
        if (bracketed != host.endsWith("]")) return null;
        String literal = bracketed ? host.substring(1, host.length() - 1) : host;
        if (literal.isEmpty()) return null;

        if (literal.indexOf(':') >= 0) {
            // HttpUrl accepts percent-encoded host text, so the raw IPv6 form is checked first.
            for (int i = 0; i < literal.length(); i++) {
                char c = literal.charAt(i);
                if (c != ':' && c != '.' && Character.digit(c, 16) < 0) return null;
            }
            try {
                // Throws for an invalid IPv6 literal (an IPv4-mapped one comes back as IPv4).
                new HttpUrl.Builder().scheme("http").host(literal);
            } catch (IllegalArgumentException e) {
                return null;
            }
        } else {
            if (bracketed) return null;
            String[] octets = literal.split("\\.", -1);
            if (octets.length != 4) return null;
            for (String octet : octets) {
                if (octet.isEmpty() || octet.length() > 3) return null;
                for (int i = 0; i < octet.length(); i++) {
                    if (octet.charAt(i) < '0' || octet.charAt(i) > '9') return null;
                }
                if (Integer.parseInt(octet) > 255) return null;
            }
        }
        try {
            // A numeric literal: parsed, never looked up.
            return InetAddress.getByName(literal);
        } catch (UnknownHostException e) {
            return null;
        }
    }
}
