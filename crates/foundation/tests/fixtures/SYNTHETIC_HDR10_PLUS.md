# Synthetic HDR10+ fixture

`synthetic_hevc_hdr10plus.mkv` was generated from 24 frames of FFmpeg's
`testsrc2=size=128x72:rate=24`. The mastering display and dynamic luminance
values come from the synthetic `RealHevcColorFixture` recipe in
`../internal/video_detection.rs`. The file has no audio, external media, or
copied container metadata.

Generate the fallback with that recipe on an x265 build with HDR10+ support,
using `pools=1:frame-threads=1` in the x265 parameters and
`-map_metadata -1 -fflags +bitexact` for the output. Verify that ffprobe
reports BT.2020, SMPTE ST 2084, and typed SMPTE ST 2094-40 frame side data
before replacing the committed fixture. The fallback keeps the tests working
on x265 builds that cannot emit the dynamic metadata locally.
