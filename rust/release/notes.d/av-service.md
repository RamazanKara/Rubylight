# Service file
- **The service file stays the same across updates.** `butterpollo-service.exe` now changes only when its own code does, so after an update antivirus programs see a file they already know instead of a new one. The service starts and watches the host and rolls back interrupted updates as before; the Nucleus Co-op restore and the display driver check now run as short host tasks it starts.
- Every Rubylight executable now names its company, copyright and original file name in its file properties.
