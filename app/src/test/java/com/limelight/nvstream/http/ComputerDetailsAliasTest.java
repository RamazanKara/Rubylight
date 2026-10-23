package com.limelight.nvstream.http;

import static org.junit.Assert.*;

import org.junit.Test;

public class ComputerDetailsAliasTest {
    private static ComputerDetails host(String name) {
        ComputerDetails details = new ComputerDetails();
        details.uuid = "host";
        details.name = name;
        return details;
    }

    @Test
    public void theAliasIsShownInsteadOfTheNameUntilItIsCleared() {
        ComputerDetails details = host("DESKTOP-1");
        assertEquals("DESKTOP-1", details.displayName());
        details.alias = "Den PC";
        assertEquals("Den PC", details.displayName());
        details.alias = "";
        assertEquals("DESKTOP-1", details.displayName());
        details.alias = null;
        assertEquals("DESKTOP-1", details.displayName());
    }

    @Test
    public void aPollThatRenamesTheHostKeepsTheAliasAndCopiesCarryIt() {
        ComputerDetails details = host("DESKTOP-1");
        details.alias = "Den PC";
        details.update(host("DESKTOP-2"));
        assertEquals("DESKTOP-2", details.name);
        assertEquals("Den PC", details.alias);
        assertEquals("Den PC", new ComputerDetails(details).displayName());
    }
}
