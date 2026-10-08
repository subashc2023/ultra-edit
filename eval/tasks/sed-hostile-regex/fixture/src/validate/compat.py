"""Patterns accepted by the v1 API. Frozen: do not edit."""

import re

ORDER_ID_V1 = re.compile(r"^ORD-\d{6,8}$")
TAG_LIST_V1 = re.compile("^\\w+(?:\\|\\w+)*$")
PHONE_SUB_V1 = (r"^\+?1?[ .-]?\(?(\d{3})\)?[ .-]?(\d{3})[ .-]?(\d{4})$", r"(\1) \2-\3")
