import json

data = {
    "JSONInfo": {
        "HDR10plusProfile": "B",
        "Version": "1.0"
    },
    "SceneInfo": [
        {
            "BezierCurveData": {
                "Anchors": [
                    0, 255, 511, 767, 1023, 1023, 1023, 1023, 1023, 1023, 1023, 1023, 1023, 1023, 1023
                ],
                "KneePointX": 17,
                "KneePointY": 64
            },
            "LuminanceParameters": {
                "AverageRGB": 100,
                "LuminanceDistributions": {
                    "DistributionIndex": [
                        1, 5, 10, 25, 50, 75, 90, 95, 99
                    ],
                    "DistributionValues": [
                        1, 5, 10, 25, 50, 75, 90, 95, 99
                    ]
                },
                "MaxScl": [
                    10000, 10000, 10000
                ]
            },
            "NumberOfWindows": 1,
            "SceneFrameIndex": 0,
            "SequenceFrameIndex": 0,
            "TargetedSystemDisplayMaximumLuminance": 400
        }
    ]
}

with open("debug/dummy_hdr10plus.json", "w") as f:
    json.dump(data, f, indent=2)
