package com.limelight.binding.video;

import java.io.BufferedReader;
import java.io.FileReader;
import java.io.FileWriter;
import java.util.LinkedHashMap;
import java.util.Locale;
import java.util.Map;

// Standalone adapter for measured native timestamps; no Android runtime or invented receive/render times.
public final class PyroWaveBenchmark {
    public static void main(String[] args) throws Exception {
        Map<String, FrameLatencyStats> runs = new LinkedHashMap<>();
        try (BufferedReader input = new BufferedReader(new FileReader(args[0]))) {
            String line;
            while ((line = input.readLine()) != null) {
                if (!line.matches("^(420|444),(compute|fragment),.*")) continue;
                String[] fields = line.split(",");
                FrameLatencyStats stats = runs.computeIfAbsent(fields[0] + "-" + fields[1], key -> new FrameLatencyStats());
                int frame = Integer.parseInt(fields[2]);
                long start = Long.parseLong(fields[3]);
                long end = Long.parseLong(fields[4]);
                stats.onDecoderInput(frame, frame, 0, 0, start, (char) 0);
                stats.onDecoderOutput(0, frame, end);
                stats.onOutputReleased(0, end, true, false);
            }
        }
        if (runs.size() != 4) throw new IllegalArgumentException("Expected four completed benchmark runs");
        for (Map.Entry<String, FrameLatencyStats> run : runs.entrySet()) {
            double[] measured = run.getValue().summarize()[1];
            if (measured[0] != 60) throw new IllegalArgumentException("Expected 60 completed frames per run");
            System.out.printf(Locale.ROOT, "%s: n=%.0f avg=%.6f p95=%.6f p99=%.6f ms%n",
                    run.getKey(), measured[0], measured[1], measured[2], measured[3]);
            try (FileWriter csv = new FileWriter(args[1] + "/" + run.getKey() + ".csv")) {
                csv.write(FrameLatencyStats.CSV_HEADER);
                run.getValue().writeCsv(csv);
            }
        }
    }
}
