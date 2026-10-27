package com.limelight.files;

import com.limelight.LimeLog;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nvstream.jni.FileTransferBridge;

/** The transfer list of one screen: {@link FileTransferBridge} in the app, a fake in tests. */
public interface FileTransferEngine {
    long create();

    void destroy(long handle);

    long queueUpload(long handle, String localName, long size);

    long queueDownload(long handle, String name, String localName, long size);

    boolean cancel(long handle, long id);

    void cancelAll(long handle);

    boolean retry(long handle, long id);

    void clearFinished(long handle);

    String snapshot(long handle);

    String startNext(long handle);

    /** Blocks on HTTPS. */
    String list(HostEndpoint host);

    /** Blocks on HTTPS; takes over {@code fd}. */
    String run(long handle, long id, HostEndpoint host, int fd, FileTransferBridge.Progress progress);

    /** The Rust client, or null when the library is not loaded or lacks file transfer. */
    static FileTransferEngine jni() {
        if (!FileTransferBridge.isAvailable()) {
            return null;
        }
        try {
            // Checks that this build of the library has the bridge before the screen relies on it.
            FileTransferBridge.destroy(FileTransferBridge.create());
        } catch (UnsatisfiedLinkError e) {
            LimeLog.warning("File transfer unavailable: " + e.getMessage());
            return null;
        }
        return new FileTransferEngine() {
            @Override public long create() { return FileTransferBridge.create(); }
            @Override public void destroy(long handle) { FileTransferBridge.destroy(handle); }
            @Override public long queueUpload(long handle, String localName, long size) {
                return FileTransferBridge.queueUpload(handle, localName, size);
            }
            @Override public long queueDownload(long handle, String name, String localName, long size) {
                return FileTransferBridge.queueDownload(handle, name, localName, size);
            }
            @Override public boolean cancel(long handle, long id) { return FileTransferBridge.cancel(handle, id); }
            @Override public void cancelAll(long handle) { FileTransferBridge.cancelAll(handle); }
            @Override public boolean retry(long handle, long id) { return FileTransferBridge.retry(handle, id); }
            @Override public void clearFinished(long handle) { FileTransferBridge.clearFinished(handle); }
            @Override public String snapshot(long handle) { return FileTransferBridge.snapshot(handle); }
            @Override public String startNext(long handle) { return FileTransferBridge.startNext(handle); }
            @Override public String list(HostEndpoint host) {
                return FileTransferBridge.list(host.address, host.httpPort, host.httpsPort, host.serverCertificatePem,
                        host.uniqueId, host.clientCertificateDer, host.clientPrivateKeyPkcs8);
            }
            @Override public String run(long handle, long id, HostEndpoint host, int fd,
                                        FileTransferBridge.Progress progress) {
                return FileTransferBridge.run(handle, id, host.address, host.httpPort, host.httpsPort,
                        host.serverCertificatePem, host.uniqueId, host.clientCertificateDer,
                        host.clientPrivateKeyPkcs8, fd, progress);
            }
        };
    }
}
