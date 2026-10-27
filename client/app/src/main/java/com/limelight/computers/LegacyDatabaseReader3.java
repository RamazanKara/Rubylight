package com.limelight.computers;

import android.content.Context;
import android.database.Cursor;
import android.database.sqlite.SQLiteDatabase;
import android.database.sqlite.SQLiteException;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvHTTP;

import java.io.ByteArrayInputStream;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.LinkedList;
import java.util.List;

public class LegacyDatabaseReader3 {
    private static final String COMPUTER_DB_NAME = "computers3.db";
    private static final String COMPUTER_TABLE_NAME = "Computers";

    private static final char ADDRESS_DELIMITER = ';';
    private static final char PORT_DELIMITER = '_';

    private static String readNonEmptyString(String input) {
        if (input.isEmpty()) {
            return null;
        }

        return input;
    }

    private static ComputerDetails.AddressTuple splitAddressToTuple(String input) {
        if (input == null) {
            return null;
        }

        int delimiter = input.lastIndexOf(PORT_DELIMITER);
        try {
            if (delimiter < 0) {
                return new ComputerDetails.AddressTuple(input, NvHTTP.DEFAULT_HTTP_PORT);
            }
            return new ComputerDetails.AddressTuple(input.substring(0, delimiter),
                    Integer.parseInt(input.substring(delimiter + 1)));
        } catch (IllegalArgumentException e) {
            return null;
        }
    }
    private static String splitTupleToAddress(ComputerDetails.AddressTuple tuple) {
        return tuple.address+PORT_DELIMITER+tuple.port;
    }

    private static ComputerDetails getComputerFromCursor(Cursor c) {
        ComputerDetails details = new ComputerDetails();

        details.uuid = c.getString(0);
        details.name = c.getString(1);

        String[] addresses = c.getString(2) == null ? new String[0] : c.getString(2).split(""+ADDRESS_DELIMITER, -1);

        details.localAddress = addresses.length > 0 ? splitAddressToTuple(readNonEmptyString(addresses[0])) : null;
        details.remoteAddress = addresses.length > 1 ? splitAddressToTuple(readNonEmptyString(addresses[1])) : null;
        details.manualAddress = addresses.length > 2 ? splitAddressToTuple(readNonEmptyString(addresses[2])) : null;
        details.ipv6Address = addresses.length > 3 ? splitAddressToTuple(readNonEmptyString(addresses[3])) : null;

        // External port is persisted in the remote address field
        if (details.remoteAddress != null) {
            details.externalPort = details.remoteAddress.port;
        }
        else {
            details.externalPort = NvHTTP.DEFAULT_HTTP_PORT;
        }

        details.macAddress = c.getString(3);

        try {
            byte[] derCertData = c.getBlob(4);

            if (derCertData != null) {
                details.serverCert = (X509Certificate) CertificateFactory.getInstance("X.509")
                        .generateCertificate(new ByteArrayInputStream(derCertData));
            }
        } catch (CertificateException e) {
            e.printStackTrace();
        }

        // This signifies we don't have dynamic state (like pair state)
        details.state = ComputerDetails.State.UNKNOWN;

        return details;
    }

    public static List<ComputerDetails> getAllComputers(SQLiteDatabase computerDb) {
        try (final Cursor c = computerDb.rawQuery("SELECT * FROM "+COMPUTER_TABLE_NAME, null)) {
            LinkedList<ComputerDetails> computerList = new LinkedList<>();
            while (c.moveToNext()) {
                ComputerDetails details = getComputerFromCursor(c);

                // If a critical field is corrupt or missing, skip the database entry
                if (details.uuid == null) {
                    continue;
                }

                computerList.add(details);
            }

            return computerList;
        }
    }

    public static List<ComputerDetails> migrateAllComputers(Context c) {
        try (final SQLiteDatabase computerDb = SQLiteDatabase.openDatabase(
                c.getDatabasePath(COMPUTER_DB_NAME).getPath(),
                null, SQLiteDatabase.OPEN_READONLY)
        ) {
            // Open the existing database
            return getAllComputers(computerDb);
        } catch (SQLiteException e) {
            return new LinkedList<ComputerDetails>();

        }
    }
}
