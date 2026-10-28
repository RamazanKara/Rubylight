# FEC loss reports in the stream timings
- The host log's 5 s stream timings count the client's once-a-second FEC loss reports (`fec_loss_reports`, and `fec_loss_refused_reports` for ones it refused). `fec_reports` counts only per-frame FEC status, which Nightjar doesn't send, so it stayed 0 while the loss reports moved the FEC share.
