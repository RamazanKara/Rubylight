package com.limelight;

import java.io.IOException;
import java.util.logging.FileHandler;
import java.util.logging.Logger;

public class LimeLog {
    private static final Logger LOGGER = Logger.getLogger(LimeLog.class.getName());
    private static final com.limelight.utils.RedactedLog REPORT_LOG = new com.limelight.utils.RedactedLog();

    public static void info(String msg) {
        REPORT_LOG.add("INFO", msg);
        LOGGER.info(msg);
    }
    
    public static void warning(String msg) {
        REPORT_LOG.add("WARNING", msg);
        LOGGER.warning(msg);
    }
    
    public static void severe(String msg) {
        REPORT_LOG.add("SEVERE", msg);
        LOGGER.severe(msg);
    }
    
    public static void setFileHandler(String fileName) throws IOException {
        LOGGER.addHandler(new FileHandler(fileName));
    }

    public static String getRedactedLog() {
        return REPORT_LOG.snapshot();
    }
}
