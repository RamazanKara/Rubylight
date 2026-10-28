# Controller release warning
- No more "Controller release failed (0x80070016)" warning at every stream start with a controller connected. The host no longer sends a neutral state to a virtual pad it plugged in moments earlier, which holds nothing yet. A failed release of a pad that was used still warns.
