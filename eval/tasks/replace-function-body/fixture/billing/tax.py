"""Tax rates by tax code.

Rates are loaded from ``tax_rates.toml`` in production; ``default_tax_table``
returns the built-in table used by tests and local development.
"""

from __future__ import annotations

import dataclasses
from decimal import Decimal
from typing import Mapping

DEFAULT_RATES = {
    "standard": Decimal("0.20"),
    "reduced": Decimal("0.05"),
    "zero": Decimal("0"),
    "exempt": Decimal("0"),
}


class UnknownTaxCode(KeyError):
    """Raised when a line uses a tax code the table does not define."""


@dataclasses.dataclass(frozen=True)
class TaxTable:
    region: str
    rates: Mapping[str, Decimal]

    def rate_for(self, code: str) -> Decimal:
        try:
            return self.rates[code]
        except KeyError:
            raise UnknownTaxCode(f"{self.region}: no rate for tax code {code!r}") from None

    def with_rate(self, code: str, rate: Decimal) -> "TaxTable":
        return TaxTable(self.region, {**self.rates, code: rate})


def default_tax_table() -> TaxTable:
    return TaxTable("default", dict(DEFAULT_RATES))


def table_for_region(region: str, overrides: Mapping[str, Mapping[str, str]]) -> TaxTable:
    """Return the default table with ``overrides[region]`` applied."""
    table = default_tax_table()
    for code, rate in overrides.get(region, {}).items():
        table = table.with_rate(code, Decimal(rate))
    return TaxTable(region, dict(table.rates))
