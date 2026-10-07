"""Compiled validation patterns shared by the API and the CLI."""

import re

# Raw strings: one backslash per regex escape.
SLUG = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$")
ORDER_ID = re.compile(r"^(?:ORD|RET)-\d{8,10}$")
PHONE = re.compile(r"^\+?\d{1,3}[ .-]?\(?\d{3}\)?[ .-]?\d{3}[ .-]?\d{4}$")
UNIX_PATH = re.compile(r"^(?:/[^/\0]+){1,32}/?$")
HEX_COLOR = re.compile(r"^#(?:[0-9a-fA-F]{3}){1,2}$")

# Legacy constants kept as normal strings: every backslash is doubled.
LEGACY_ORDER_ID = re.compile("^ORD-\\d{6,8}$")
WIN_PATH = re.compile("^[A-Za-z]:\\\\(?:[^\\\\/:*?\"<>|]+\\\\)*[^\\\\/:*?\"<>|]*$")
TAG_LIST = re.compile("^[\\w.-]+(?:\\|[\\w.-]+){0,15}$")

# Substitutions applied by the helpers below; \1 refers to the first group.
PHONE_SUB = (r"^\+?1?[ .-]?\(?(\d{3})\)?[ .-]?(\d{3})[ .-]?(\d{4})$", r"+1 \1-\2-\3")
DUP_SLASH_SUB = ("/{2,}", "/")


def normalize_phone(value: str) -> str:
    pattern, replacement = PHONE_SUB
    return re.sub(pattern, replacement, value.strip())


def collapse_slashes(path: str) -> str:
    pattern, replacement = DUP_SLASH_SUB
    return re.sub(pattern, replacement, path)
