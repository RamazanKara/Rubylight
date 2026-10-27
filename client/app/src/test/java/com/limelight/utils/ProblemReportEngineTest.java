package com.limelight.utils;

import com.limelight.nvstream.jni.StreamEngine;

import org.junit.Test;

import static org.junit.Assert.*;

public class ProblemReportEngineTest {
    @Test public void reportNamesTheEngineOfTheLastStream() {
        String section = ProblemReport.engineSection();
        assertTrue(section.startsWith("Streaming engine for the last stream:\n"));
        assertTrue(section.contains(StreamEngine.summary()));
        assertTrue(section.endsWith("\n\n"));
    }
}
