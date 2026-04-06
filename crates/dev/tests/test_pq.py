import cv2
import OpenEXR
import Imath
import numpy as np


def read_exr(filepath):
    f = OpenEXR.InputFile(filepath)
    dw = f.header()["dataWindow"]
    size = (dw.max.x - dw.min.x + 1, dw.max.y - dw.min.y + 1)
    FLOAT = Imath.PixelType(Imath.PixelType.FLOAT)
    channels = f.channels(["R", "G", "B"], FLOAT)
    img = np.zeros((size[1], size[0], 3), dtype=np.float32)
    img[:, :, 0] = np.frombuffer(channels[0], dtype=np.float32).reshape(
        size[1], size[0]
    )
    img[:, :, 1] = np.frombuffer(channels[1], dtype=np.float32).reshape(
        size[1], size[0]
    )
    img[:, :, 2] = np.frombuffer(channels[2], dtype=np.float32).reshape(
        size[1], size[0]
    )
    return img


def linear_to_pq(linear_img, intensity_target=10000.0):
    # linear_img is in range [0, 1] representing [0, intensity_target] nits?
    # Wait, the HDR synthesis outputs linear values where 1.0 = SDR white (e.g. 203 nits)
    # Let's scale linear_img so that 1.0 = SDR white. PQ expects 1.0 = 10000 nits.
    # If intensity_target is the max nits, maybe SDR white is at some level.
    # Apple uses 203 nits for SDR white.
    # So linear = 1.0 means 203 nits.
    # To convert to PQ, L = nits / 10000.
    L = (linear_img * 203.0) / 10000.0
    L = np.clip(L, 0.0, 1.0)

    m1 = 2610 / 16384
    m2 = 2523 / 32
    c1 = 3424 / 4096
    c2 = 2413 / 128
    c3 = 2392 / 128

    Lm = np.power(L, m1)
    N = np.power((c1 + c2 * Lm) / (1 + c3 * Lm), m2)
    return N


img = read_exr("debug/UltraHDR_Synthesized_Samples/tmp_hdr.exr")
pq_img = linear_to_pq(img)
pq_16 = (pq_img * 65535.0).astype(np.uint16)
cv2.imwrite("debug/UltraHDR_Synthesized_Samples/tmp_pq.png", pq_16[:, :, ::-1])

print("Saved tmp_pq.png")
