# Failure reasons
- **Clients learn why a stream failed.** A stream that ends on an encoder error now tells Moonlight it was the encoder, so stock Moonlight shows its encoder advice instead of a bare "Error code: 80004005". The host keeps its own words for the failure (for example that HDR is on but the display is not in HDR), and Nightjar shows them in its error dialog.
