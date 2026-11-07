## en
- With "Prioritize stream traffic" on, the thread that feeds the video decoder now gets the decoder's priority on every decoder, including the ones that decode as packets arrive, so a busy phone delays frames less.

## desktop
- On Windows the stream's receive, decode, audio and control threads now get a raised priority, so other busy programs delay frames less.
