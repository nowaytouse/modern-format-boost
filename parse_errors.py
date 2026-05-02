import sys
import re

def strip_ansi(text):
    return re.sub(r'\x1B\[[0-9;]*[mK]', '', text)

errors = []
current_error = []

for line in sys.stdin:
    stripped = strip_ansi(line)
    if stripped.startswith('error') or stripped.startswith('warning:'):
        if current_error:
            errors.append(''.join(current_error))
        current_error = [line]
    elif current_error:
        current_error.append(line)

if current_error:
    errors.append(''.join(current_error))

for error in errors:
    stripped_error = strip_ansi(error)
    if "too_many_lines" not in stripped_error and "error" in stripped_error:
        print(error)
