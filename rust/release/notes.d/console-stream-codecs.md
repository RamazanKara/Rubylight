# Console stream codecs
- **Stream in the browser picks the codec, resolution and frame rate.** The Stream page now offers AV1, HEVC and H.264 at every resolution and frame rate the host's encoder takes, limited to what the browser can receive. It starts with a codec the browser decodes in hardware at 1080p60 and remembers your choice (issue 67).
- **Firefox streams in the browser.** Firefox refused the host's offer with "SIPCC Failed to parse SDP: Min is not smaller than max". The page now leaves out the Samsung TV line that caused it (issue 66).
