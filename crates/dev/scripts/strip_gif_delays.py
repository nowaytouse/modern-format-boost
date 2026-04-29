import sys


def strip_gce(input_path, output_path):
    with open(input_path, "rb") as f:
        data = bytearray(f.read())

    # Simple block parser to find and remove 0x21 0xF9 0x04 blocks
    i = 6
    if data[0:6] not in (b"GIF87a", b"GIF89a"):
        print("Invalid GIF")
        return

    lsd_packed = data[10]
    has_gct = (lsd_packed & 0x80) != 0
    gct_size = lsd_packed & 0x07
    i = 13
    if has_gct:
        i += 3 * (2 ** (gct_size + 1))

    out_data = bytearray(data[:i])

    while i < len(data):
        if data[i] == 0x21 and i + 1 < len(data) and data[i + 1] == 0xF9:
            # Graphic Control Extension! Length is 4 bytes + block terminator
            if i + 7 < len(data) and data[i + 7] == 0x00:
                print(f"Stripping GCE cleanly at {i}")
                i += 8
            else:
                block_len = data[i + 2]
                i += 3 + block_len
                # find 0x00 terminator
                while True:
                    size = data[i]
                    i += 1
                    if size == 0:
                        break
                    i += size
        elif data[i] == 0x21:
            out_data.append(data[i])
            i += 1
            out_data.append(data[i])
            i += 1
            while True:
                size = data[i]
                out_data.append(size)
                i += 1
                if size == 0:
                    break
                out_data.extend(data[i : i + size])
                i += size
        elif data[i] == 0x2C:
            out_data.append(data[i])
            i += 1
            out_data.extend(data[i : i + 8])
            i += 8
            packed = data[i]
            out_data.append(packed)
            i += 1
            if (packed & 0x80) != 0:
                color_table_size = 2 ** ((packed & 0x07) + 1) * 3
                out_data.extend(data[i : i + color_table_size])
                i += color_table_size
            # LZW minimum code size
            out_data.append(data[i])
            i += 1
            # Image data blocks
            while True:
                size = data[i]
                out_data.append(size)
                i += 1
                if size == 0:
                    break
                out_data.extend(data[i : i + size])
                i += size
        elif data[i] == 0x3B:
            out_data.append(data[i])
            break
        else:
            out_data.append(data[i])
            i += 1

    with open(output_path, "wb") as f:
        f.write(out_data)


strip_gce(sys.argv[1], sys.argv[2])
