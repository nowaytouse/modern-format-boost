import re

with open('crates/shared_utils/src/numeric_cast.rs', 'r') as f:
    content = f.read()

# Delete functions ending in _sat. They usually look like:
# pub fn f64_to_u64_sat(v: f64) -> u64 { ... }
# with some comments and attributes above them.

pattern = re.compile(r'(///.*?\n)*#\[inline\]\n#\[must_use\]\npub fn [a-z0-9_]+_sat\([^)]+\) -> [a-z0-9]+ \{.*?^\}', re.MULTILINE | re.DOTALL)

new_content = pattern.sub('', content)

with open('crates/shared_utils/src/numeric_cast.rs', 'w') as f:
    f.write(new_content)
