## en
- The performance overlay's Display wait and Client latency now end when Nightjar hands a frame to Android, as they already did with the Vulkan renderer and in Nightjar for Windows and Linux. Android's compositor and the screen's own delay come after that and are left out; they stay in the log and the latency CSV.
- Debug timings show the average, median and p95, so one slow frame no longer dominates the row.
