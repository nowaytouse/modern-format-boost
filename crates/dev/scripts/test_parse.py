info = """Canvas size: 512 x 512
Features present: animation transparency
Background color : 0xFFFFFF00
Number of frames: 10
No.: width height alpha x_offset y_offset duration   dispose blend image_size  compression
  1:   512   512   yes        0        0       80      none blend      18563    lossless
  2:   512   512   yes        0        0       120      none blend       2023    lossless"""

durations = []
parsing_frames = False
for line in info.splitlines():
    if "No.: width height" in line:
        parsing_frames = True
        continue
    if parsing_frames:
        parts = line.split()
        if len(parts) >= 7 and parts[0].endswith(":"):
            try:
                durations.append(int(parts[6]))
            except ValueError:
                pass
print(durations)
