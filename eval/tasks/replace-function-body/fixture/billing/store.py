"""In-memory invoice store used by the API handlers.

The production deployment swaps this for the Postgres-backed store in
``billing_pg``; both expose the same methods.
"""

from __future__ import annotations

import datetime as dt
import threading
from typing import ClassVar

from billing.invoice import Invoice, InvoiceTotals, compute_totals


class NotFound(LookupError):
    pass


class InvoiceStore:
    _shared: ClassVar["InvoiceStore | None"] = None
    _lock: ClassVar[threading.Lock] = threading.Lock()

    def __init__(self) -> None:
        self._invoices: dict[str, Invoice] = {}
        self._totals: dict[str, InvoiceTotals] = {}
        self._audit: list[tuple[str, str, dt.datetime]] = []
        self._tax_overrides: dict[str, dict[str, str]] = {}

    @classmethod
    def shared(cls) -> "InvoiceStore":
        with cls._lock:
            if cls._shared is None:
                cls._shared = cls()
            return cls._shared

    def exists(self, number: str) -> bool:
        return number in self._invoices

    def get(self, number: str) -> Invoice:
        try:
            return self._invoices[number]
        except KeyError:
            raise NotFound(number) from None

    def save(self, invoice: Invoice, *, created_by: str, created_at: dt.datetime) -> None:
        self._invoices[invoice.number] = invoice
        self._totals.pop(invoice.number, None)
        self._audit.append((invoice.number, created_by, created_at))

    def cached_totals(self, number: str) -> InvoiceTotals:
        if number not in self._totals:
            self._totals[number] = compute_totals(self.get(number))
        return self._totals[number]

    def tax_overrides(self) -> dict[str, dict[str, str]]:
        return self._tax_overrides
