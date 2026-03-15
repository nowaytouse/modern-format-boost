#!/bin/bash
echo "🔍 Verifying HEIC Configuration..."
echo "Memory limit: $(grep -A 1 'Set to.*GB' shared_utils/src/image_heic_analysis.rs | grep set_max_total_memory)"
echo "ipco limit: $(grep set_max_children_per_box shared_utils/src/image_heic_analysis.rs)"
echo "Features: $(grep -A 2 '^\[features\]' shared_utils/Cargo.toml)"
