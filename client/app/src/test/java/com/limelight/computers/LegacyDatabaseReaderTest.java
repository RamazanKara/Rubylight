package com.limelight.computers;

import android.database.Cursor;

import com.limelight.nvstream.http.ComputerDetails;
import org.junit.Test;

import java.lang.reflect.Method;
import java.lang.reflect.Proxy;

import static org.junit.Assert.*;

public class LegacyDatabaseReaderTest {
    private static ComputerDetails read(Class<?> reader, Object... columns) throws Exception {
        Cursor cursor = (Cursor) Proxy.newProxyInstance(Cursor.class.getClassLoader(), new Class<?>[] {Cursor.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("getColumnCount")) return columns.length;
                    if (method.getName().equals("getString") || method.getName().equals("getBlob")) {
                        return columns[(Integer) args[0]];
                    }
                    throw new AssertionError(method.getName());
                });
        Method method = reader.getDeclaredMethod("getComputerFromCursor", Cursor.class);
        method.setAccessible(true);
        return (ComputerDetails) method.invoke(null, cursor);
    }

    @Test
    public void firstSchemaAllowsMissingAddresses() throws Exception {
        ComputerDetails details = read(LegacyDatabaseReader.class, "PC", "uuid", null, null, "00:11:22:33:44:55");
        assertNull(details.localAddress);
        assertNull(details.remoteAddress);
    }

    @Test
    public void secondSchemaAllowsMissingAddresses() throws Exception {
        ComputerDetails details = read(LegacyDatabaseReader2.class, "uuid", "PC", "192.168.1.10", null, null, null);
        assertEquals("192.168.1.10", details.localAddress.address);
        assertNull(details.remoteAddress);
        assertNull(details.manualAddress);
    }

    @Test
    public void thirdSchemaKeepsUnderscoresInHostnames() throws Exception {
        ComputerDetails details = read(LegacyDatabaseReader3.class, "uuid", "PC", ";;my_pc.local_47989;", null, null);
        assertEquals(new ComputerDetails.AddressTuple("my_pc.local", 47989), details.manualAddress);
    }

    @Test
    public void thirdSchemaKeepsValidAddressesWhenOnePortIsCorrupt() throws Exception {
        ComputerDetails details = read(LegacyDatabaseReader3.class, "uuid", "PC",
                "192.168.1.10_47989;example.test_bad;[2001:db8::1]_47990;", null, null);
        assertEquals("192.168.1.10", details.localAddress.address);
        assertNull(details.remoteAddress);
        assertEquals(47990, details.manualAddress.port);
    }
}
