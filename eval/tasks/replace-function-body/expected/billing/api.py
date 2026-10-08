"""HTTP handlers for the billing service.

Handlers take a parsed ``Request`` and return ``(status, body)`` tuples; the
routing table at the bottom of the module is mounted by ``billing.server``.
"""

from __future__ import annotations

import dataclasses
import datetime as dt
from typing import Callable

from billing.invoice import (
    InvoiceError,
    compute_totals,
    format_invoice_summary,
    invoice_from_dict,
    invoice_to_dict,
    render_invoice_text,
    validate_invoice,
)
from billing.store import InvoiceStore, NotFound
from billing.tax import TaxTable, UnknownTaxCode, table_for_region

Response = tuple[int, dict[str, object]]


@dataclasses.dataclass(frozen=True)
class Request:
    method: str
    path: str
    params: dict[str, str]
    body: dict[str, object]
    region: str = "default"
    user: str = "anonymous"


def resolve_tax_table(region: str) -> TaxTable:
    return table_for_region(region, InvoiceStore.shared().tax_overrides())


def _error(status: int, message: str) -> Response:
    return status, {"error": message}


def get_invoice(request: Request) -> Response:
    store = InvoiceStore.shared()
    try:
        invoice = store.get(request.params["number"])
    except NotFound:
        return _error(404, "invoice not found")
    tax_table = resolve_tax_table(request.region)
    totals = compute_totals(invoice, tax_table=tax_table, rounding="half_even")
    return 200, {"invoice": invoice_to_dict(invoice), "totals": totals.as_dict()}


def preview_invoice(request: Request) -> Response:
    """Return totals for an unsaved draft. Drafts always use the default tax table."""
    try:
        draft = invoice_from_dict(request.body)
    except InvoiceError as error:
        return _error(400, str(error))
    totals = compute_totals(draft)
    return 200, {"summary": format_invoice_summary(draft, totals), "totals": totals.as_dict()}


def create_invoice(request: Request) -> Response:
    try:
        invoice = invoice_from_dict(request.body)
    except InvoiceError as error:
        return _error(400, str(error))
    problems = validate_invoice(invoice)
    if problems:
        return 422, {"error": "invalid invoice", "problems": problems}
    store = InvoiceStore.shared()
    if store.exists(invoice.number):
        return _error(409, f"invoice {invoice.number} already exists")
    try:
        totals = compute_totals(
            invoice,
            tax_table=resolve_tax_table(request.region),
            rounding="half_even",
        )
    except UnknownTaxCode as error:
        return _error(422, str(error))
    store.save(invoice, created_by=request.user, created_at=dt.datetime.now(dt.timezone.utc))
    return 201, {"invoice": invoice_to_dict(invoice), "totals": totals.as_dict()}


def render_invoice(request: Request) -> Response:
    store = InvoiceStore.shared()
    try:
        invoice = store.get(request.params["number"])
    except NotFound:
        return _error(404, "invoice not found")
    totals = store.cached_totals(invoice.number)
    width = int(request.params.get("width", "72"))
    return 200, {"text": render_invoice_text(invoice, totals, width=width)}


ROUTES: dict[tuple[str, str], Callable[[Request], Response]] = {
    ("GET", "/invoices/{number}"): get_invoice,
    ("GET", "/invoices/{number}/text"): render_invoice,
    ("POST", "/invoices"): create_invoice,
    ("POST", "/invoices/preview"): preview_invoice,
}
