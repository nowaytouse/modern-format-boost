            // APP1 (0xE1): XMP Extended
            else if payload.starts_with(b"http://ns.adobe.com/xmp/extension/\0")
                && payload.len() > 35 + 32 + 8
            {
                let xmp = String::from_utf8_lossy(&payload[35 + 32 + 8..]).to_string();
                xmp_blocks.push(xmp);
            }
            pos += 2 + seg_len;
        } else {
            pos += 1;
        }
