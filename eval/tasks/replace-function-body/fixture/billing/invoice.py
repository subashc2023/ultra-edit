"""Invoice model and arithmetic for the billing service.

Amounts are stored as :class:`decimal.Decimal` values in the invoice currency.
Every public function in this module is pure: it never mutates its arguments
and never performs I/O. Persistence lives in ``billing.store`` and the HTTP
layer lives in ``billing.api``.
"""

from __future__ import annotations

import dataclasses
import datetime as dt
from collections import defaultdict
from decimal import ROUND_HALF_EVEN, ROUND_HALF_UP, Decimal, InvalidOperation
from typing import Iterable, Iterator, Mapping, Sequence

from billing.tax import TaxTable, default_tax_table

__all__ = [
    "Adjustment",
    "Invoice",
    "InvoiceError",
    "InvoiceTotals",
    "LineItem",
    "aging_buckets",
    "apply_credit",
    "compute_totals",
    "format_amount",
    "format_invoice_summary",
    "invoice_from_dict",
    "invoice_to_dict",
    "iter_overdue",
    "merge_invoices",
    "next_invoice_number",
    "prorate_line_item",
    "render_invoice_text",
    "split_invoice",
    "summarize_by_customer",
    "validate_invoice",
]

CENT = Decimal("0.01")
ZERO = Decimal("0")
ROUNDING_MODES = {
    "half_up": ROUND_HALF_UP,
    "half_even": ROUND_HALF_EVEN,
}
SUPPORTED_CURRENCIES = frozenset({"USD", "EUR", "GBP", "CAD", "AUD", "JPY"})
ZERO_DECIMAL_CURRENCIES = frozenset({"JPY"})
CURRENCY_SYMBOLS = {
    "USD": "$",
    "EUR": "€",
    "GBP": "£",
    "CAD": "CA$",
    "AUD": "A$",
    "JPY": "¥",
}
MAX_LINE_ITEMS = 500
AGING_BUCKETS = (0, 30, 60, 90)


class InvoiceError(ValueError):
    """Raised when an invoice cannot be built, validated, or totalled."""


@dataclasses.dataclass(frozen=True)
class Adjustment:
    """A signed amount applied to one line item (discount, surcharge, credit)."""

    kind: str
    amount: Decimal
    reason: str = ""

    @property
    def is_discount(self) -> bool:
        return self.amount < ZERO


@dataclasses.dataclass(frozen=True)
class LineItem:
    """One billable row: ``quantity`` units of ``sku`` at ``unit_price``."""

    sku: str
    description: str
    quantity: int
    unit_price: Decimal
    tax_code: str = "standard"
    tax_exempt: bool = False
    adjustments: tuple[Adjustment, ...] = ()

    @property
    def gross(self) -> Decimal:
        return self.unit_price * self.quantity

    @property
    def adjustment_total(self) -> Decimal:
        return sum((adjustment.amount for adjustment in self.adjustments), ZERO)

    @property
    def net(self) -> Decimal:
        return self.gross + self.adjustment_total


@dataclasses.dataclass(frozen=True)
class Invoice:
    """An issued invoice. Instances are immutable; use ``dataclasses.replace``."""

    number: str
    customer_id: str
    currency: str
    issued_on: dt.date
    due_on: dt.date
    lines: tuple[LineItem, ...] = ()
    credits: tuple[Decimal, ...] = ()
    notes: str = ""

    def __post_init__(self) -> None:
        if self.currency not in SUPPORTED_CURRENCIES:
            raise InvoiceError(f"unsupported currency {self.currency!r}")
        if self.due_on < self.issued_on:
            raise InvoiceError(f"invoice {self.number}: due date precedes issue date")

    @property
    def line_count(self) -> int:
        return len(self.lines)

    def is_overdue(self, today: dt.date) -> bool:
        return today > self.due_on


@dataclasses.dataclass
class InvoiceTotals:
    """The computed amounts of one invoice, all rounded to the cent."""

    subtotal: Decimal = ZERO
    discounts: Decimal = ZERO
    surcharges: Decimal = ZERO
    tax: Decimal = ZERO
    credits: Decimal = ZERO
    total: Decimal = ZERO
    tax_by_code: dict[str, Decimal] = dataclasses.field(default_factory=dict)
    warnings: list[str] = dataclasses.field(default_factory=list)

    def as_dict(self) -> dict[str, object]:
        return {
            "subtotal": str(self.subtotal),
            "discounts": str(self.discounts),
            "surcharges": str(self.surcharges),
            "tax": str(self.tax),
            "credits": str(self.credits),
            "total": str(self.total),
            "tax_by_code": {code: str(amount) for code, amount in self.tax_by_code.items()},
            "warnings": list(self.warnings),
        }


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _to_decimal(value: object, *, field: str) -> Decimal:
    """Convert ``value`` to ``Decimal`` or raise :class:`InvoiceError`."""
    if isinstance(value, Decimal):
        return value
    if isinstance(value, float):
        raise InvoiceError(f"{field}: floats are not accepted, pass a string")
    try:
        return Decimal(str(value))
    except (InvalidOperation, ValueError) as error:
        raise InvoiceError(f"{field}: {value!r} is not a decimal amount") from error


def _round_cents(value: Decimal) -> Decimal:
    """Round ``value`` half-up to the cent."""
    return value.quantize(CENT, rounding=ROUND_HALF_UP)


def _quantize(value: Decimal, rounding: str = ROUND_HALF_UP) -> Decimal:
    """Round ``value`` to the cent with an explicit ``decimal`` rounding mode."""
    return value.quantize(CENT, rounding=rounding)


def _parse_date(value: object, *, field: str) -> dt.date:
    if isinstance(value, dt.date):
        return value
    try:
        return dt.date.fromisoformat(str(value))
    except ValueError as error:
        raise InvoiceError(f"{field}: {value!r} is not an ISO date") from error


def _require(mapping: Mapping[str, object], key: str, *, context: str) -> object:
    try:
        return mapping[key]
    except KeyError:
        raise InvoiceError(f"{context}: missing required field {key!r}") from None


# ---------------------------------------------------------------------------
# Serialization
# ---------------------------------------------------------------------------


def parse_adjustment(raw: Mapping[str, object], *, context: str) -> Adjustment:
    kind = str(_require(raw, "kind", context=context))
    amount = _to_decimal(_require(raw, "amount", context=context), field=f"{context}.amount")
    return Adjustment(kind=kind, amount=amount, reason=str(raw.get("reason", "")))


def parse_line_item(raw: Mapping[str, object], *, index: int) -> LineItem:
    context = f"lines[{index}]"
    quantity = _require(raw, "quantity", context=context)
    if not isinstance(quantity, int) or quantity <= 0:
        raise InvoiceError(f"{context}.quantity must be a positive integer")
    adjustments = tuple(
        parse_adjustment(item, context=f"{context}.adjustments[{position}]")
        for position, item in enumerate(raw.get("adjustments", ()))
    )
    return LineItem(
        sku=str(_require(raw, "sku", context=context)),
        description=str(raw.get("description", "")),
        quantity=quantity,
        unit_price=_to_decimal(_require(raw, "unit_price", context=context), field=f"{context}.unit_price"),
        tax_code=str(raw.get("tax_code", "standard")),
        tax_exempt=bool(raw.get("tax_exempt", False)),
        adjustments=adjustments,
    )


def invoice_from_dict(raw: Mapping[str, object]) -> Invoice:
    """Build an :class:`Invoice` from its JSON representation."""
    lines = tuple(
        parse_line_item(item, index=index) for index, item in enumerate(raw.get("lines", ()))
    )
    if len(lines) > MAX_LINE_ITEMS:
        raise InvoiceError(f"an invoice may have at most {MAX_LINE_ITEMS} lines")
    credits = tuple(
        _to_decimal(value, field=f"credits[{index}]")
        for index, value in enumerate(raw.get("credits", ()))
    )
    return Invoice(
        number=str(_require(raw, "number", context="invoice")),
        customer_id=str(_require(raw, "customer_id", context="invoice")),
        currency=str(raw.get("currency", "USD")),
        issued_on=_parse_date(_require(raw, "issued_on", context="invoice"), field="issued_on"),
        due_on=_parse_date(_require(raw, "due_on", context="invoice"), field="due_on"),
        lines=lines,
        credits=credits,
        notes=str(raw.get("notes", "")),
    )


def invoice_to_dict(invoice: Invoice) -> dict[str, object]:
    """Return the JSON representation of ``invoice``."""
    return {
        "number": invoice.number,
        "customer_id": invoice.customer_id,
        "currency": invoice.currency,
        "issued_on": invoice.issued_on.isoformat(),
        "due_on": invoice.due_on.isoformat(),
        "lines": [
            {
                "sku": line.sku,
                "description": line.description,
                "quantity": line.quantity,
                "unit_price": str(line.unit_price),
                "tax_code": line.tax_code,
                "tax_exempt": line.tax_exempt,
                "adjustments": [
                    {"kind": adj.kind, "amount": str(adj.amount), "reason": adj.reason}
                    for adj in line.adjustments
                ],
            }
            for line in invoice.lines
        ],
        "credits": [str(credit) for credit in invoice.credits],
        "notes": invoice.notes,
    }


# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------


def validate_invoice(invoice: Invoice) -> list[str]:
    """Return a list of human-readable problems; an empty list means valid."""
    problems: list[str] = []
    if not invoice.lines:
        problems.append("invoice has no line items")
    seen: set[str] = set()
    for index, line in enumerate(invoice.lines):
        if line.sku in seen:
            problems.append(f"lines[{index}]: duplicate sku {line.sku!r}")
        seen.add(line.sku)
        if line.unit_price < ZERO:
            problems.append(f"lines[{index}]: negative unit price")
        if line.net < ZERO:
            problems.append(f"lines[{index}]: adjustments exceed the line amount")
        if line.tax_exempt and line.tax_code != "exempt":
            problems.append(f"lines[{index}]: tax-exempt lines must use tax code 'exempt'")
    for index, credit in enumerate(invoice.credits):
        if credit <= ZERO:
            problems.append(f"credits[{index}]: credits must be positive")
    if invoice.currency in ZERO_DECIMAL_CURRENCIES:
        for index, line in enumerate(invoice.lines):
            if line.unit_price != line.unit_price.to_integral_value():
                problems.append(f"lines[{index}]: {invoice.currency} has no minor unit")
    return problems


# ---------------------------------------------------------------------------
# Totals
# ---------------------------------------------------------------------------


def compute_totals(invoice: Invoice, *, tax_table: TaxTable | None = None) -> InvoiceTotals:
    """Return the subtotal, discounts, tax, credits, and total of ``invoice``.

    Tax is computed per line and rounded half-up to the cent, then summed.
    Credits are applied last and never take the total below zero.
    """
    table = tax_table or default_tax_table()
    totals = InvoiceTotals()
    for line in invoice.lines:
        totals.subtotal += line.gross
        for adjustment in line.adjustments:
            if adjustment.is_discount:
                totals.discounts += adjustment.amount
            else:
                totals.surcharges += adjustment.amount
        if line.tax_exempt:
            continue
        rate = table.rate_for(line.tax_code)
        line_tax = _round_cents(line.net * rate)
        totals.tax += line_tax
        totals.tax_by_code[line.tax_code] = (
            totals.tax_by_code.get(line.tax_code, ZERO) + line_tax
        )
    totals.subtotal = _round_cents(totals.subtotal)
    totals.discounts = _round_cents(totals.discounts)
    totals.surcharges = _round_cents(totals.surcharges)
    before_credits = totals.subtotal + totals.discounts + totals.surcharges + totals.tax
    available = sum(invoice.credits, ZERO)
    if available > before_credits:
        totals.warnings.append(
            "credits exceed the invoice amount; the excess was not applied"
        )
        available = before_credits
    totals.credits = _round_cents(available)
    totals.total = _round_cents(before_credits - totals.credits)
    if invoice.currency in ZERO_DECIMAL_CURRENCIES:
        totals.total = totals.total.quantize(Decimal("1"), rounding=ROUND_HALF_UP)
    return totals


def summarize_by_customer(
    invoices: Iterable[Invoice], *, tax_table: TaxTable | None = None
) -> dict[str, dict[str, Decimal]]:
    """Return ``{customer_id: {currency: total}}`` across ``invoices``."""
    totals: dict[str, dict[str, Decimal]] = defaultdict(lambda: defaultdict(lambda: ZERO))
    for invoice in invoices:
        invoice_totals = compute_totals(invoice, tax_table=tax_table)
        totals[invoice.customer_id][invoice.currency] += invoice_totals.total
    for customer in totals.values():
        for currency, amount in customer.items():
            customer[currency] = _round_cents(amount)
    return totals


def prorate_line_item(line: LineItem, *, days_used: int, days_in_period: int) -> LineItem:
    """Return a copy of ``line`` charged for ``days_used`` of ``days_in_period``."""
    if days_in_period <= 0:
        raise InvoiceError("days_in_period must be positive")
    if not 0 <= days_used <= days_in_period:
        raise InvoiceError("days_used must be between 0 and days_in_period")
    prorated = _round_cents(line.unit_price * Decimal(days_used) / Decimal(days_in_period))
    description = f"{line.description} (prorated {days_used}/{days_in_period} days)"
    return dataclasses.replace(line, unit_price=prorated, description=description)


def apply_credit(invoice: Invoice, amount: object) -> Invoice:
    """Return a copy of ``invoice`` with one more credit of ``amount``."""
    credit = _round_cents(_to_decimal(amount, field="credit"))
    if credit <= ZERO:
        raise InvoiceError("credit must be positive")
    return dataclasses.replace(invoice, credits=invoice.credits + (credit,))


def split_invoice(invoice: Invoice, *, max_lines: int) -> list[Invoice]:
    """Split ``invoice`` into parts of at most ``max_lines`` lines each.

    Credits stay on the first part. Parts are numbered ``<number>-1``,
    ``<number>-2``, and so on.
    """
    if max_lines <= 0:
        raise InvoiceError("max_lines must be positive")
    if invoice.line_count <= max_lines:
        return [invoice]
    parts: list[Invoice] = []
    for start in range(0, invoice.line_count, max_lines):
        part_number = len(parts) + 1
        parts.append(
            dataclasses.replace(
                invoice,
                number=f"{invoice.number}-{part_number}",
                lines=invoice.lines[start : start + max_lines],
                credits=invoice.credits if part_number == 1 else (),
            )
        )
    return parts


def merge_invoices(invoices: Sequence[Invoice], *, number: str) -> Invoice:
    """Merge invoices of one customer and currency into a single invoice."""
    if not invoices:
        raise InvoiceError("nothing to merge")
    first = invoices[0]
    for other in invoices[1:]:
        if other.customer_id != first.customer_id:
            raise InvoiceError("cannot merge invoices of different customers")
        if other.currency != first.currency:
            raise InvoiceError("cannot merge invoices in different currencies")
    by_sku: dict[str, list[LineItem]] = defaultdict(list)
    for invoice in invoices:
        for line in invoice.lines:
            by_sku[line.sku].append(line)
    merged_lines = []
    for sku, lines in by_sku.items():
        if len({line.unit_price for line in lines}) == 1:
            merged_lines.append(
                dataclasses.replace(
                    lines[0],
                    quantity=sum(line.quantity for line in lines),
                    adjustments=tuple(adj for line in lines for adj in line.adjustments),
                )
            )
        else:
            merged_lines.extend(lines)
    return Invoice(
        number=number,
        customer_id=first.customer_id,
        currency=first.currency,
        issued_on=min(invoice.issued_on for invoice in invoices),
        due_on=min(invoice.due_on for invoice in invoices),
        lines=tuple(merged_lines),
        credits=tuple(credit for invoice in invoices for credit in invoice.credits),
        notes="\n".join(invoice.notes for invoice in invoices if invoice.notes),
    )


# ---------------------------------------------------------------------------
# Aging and numbering  
# ---------------------------------------------------------------------------


def iter_overdue(invoices: Iterable[Invoice], *, today: dt.date) -> Iterator[Invoice]:
    """Yield the invoices whose due date is before ``today``, oldest first."""
    yield from sorted(
        (invoice for invoice in invoices if invoice.is_overdue(today)),
        key=lambda invoice: (invoice.due_on, invoice.number),
    )


def aging_buckets(
    invoices: Iterable[Invoice], *, today: dt.date, tax_table: TaxTable | None = None
) -> dict[str, Decimal]:
    """Group overdue totals into ``current``, ``1-30``, ``31-60``, ``61-90``, ``90+``."""
    buckets = {"current": ZERO, "1-30": ZERO, "31-60": ZERO, "61-90": ZERO, "90+": ZERO}
    for invoice in invoices:
        total = compute_totals(invoice, tax_table=tax_table).total
        days = (today - invoice.due_on).days
        if days <= AGING_BUCKETS[0]:
            buckets["current"] += total
        elif days <= AGING_BUCKETS[1]:
            buckets["1-30"] += total
        elif days <= AGING_BUCKETS[2]:
            buckets["31-60"] += total
        elif days <= AGING_BUCKETS[3]:
            buckets["61-90"] += total
        else:
            buckets["90+"] += total
    return {name: _round_cents(amount) for name, amount in buckets.items()}


def next_invoice_number(existing: Iterable[str], *, prefix: str, year: int) -> str:
    """Return the next free ``<prefix>-<year>-NNNNN`` number."""
    stem = f"{prefix}-{year}-"
    highest = 0
    for number in existing:
        if not number.startswith(stem):
            continue
        suffix = number[len(stem) :].split("-", 1)[0]
        if suffix.isdigit():
            highest = max(highest, int(suffix))
    return f"{stem}{highest + 1:05d}"


# ---------------------------------------------------------------------------
# Presentation
# ---------------------------------------------------------------------------


def format_amount(amount: Decimal, currency: str) -> str:
    """Format ``amount`` with its currency symbol, e.g. ``$1,234.50``."""
    symbol = CURRENCY_SYMBOLS.get(currency, f"{currency} ")
    if currency in ZERO_DECIMAL_CURRENCIES:
        rounded = amount.quantize(Decimal("1"), rounding=ROUNDING_MODES["half_up"])
        body = f"{abs(rounded):,}"
    else:
        body = f"{abs(_round_cents(amount)):,.2f}"
    sign = "-" if amount < ZERO else ""
    return f"{sign}{symbol}{body}"


def format_invoice_summary(invoice: Invoice, totals: InvoiceTotals) -> str:
    """Return a one-line summary such as ``INV-2026-00012 acme $1,204.00``."""
    status = " (credit applied)" if totals.credits else ""
    return f"{invoice.number} {invoice.customer_id} {format_amount(totals.total, invoice.currency)}{status}"


def render_invoice_text(invoice: Invoice, totals: InvoiceTotals, *, width: int = 72) -> str:
    """Render a plain-text invoice for e-mail bodies and PDF fallbacks."""
    currency = invoice.currency
    rule = "-" * width
    out = [
        f"Invoice {invoice.number}",
        f"Customer: {invoice.customer_id}",
        f"Issued: {invoice.issued_on:%d %b %Y}    Due: {invoice.due_on:%d %b %Y}",
        rule,
    ]
    for line in invoice.lines:
        amount = format_amount(line.net, currency)
        label = f"{line.quantity} x {line.description or line.sku}"
        out.append(f"{label[: width - len(amount) - 1]:<{width - len(amount)}}{amount}")
        for adjustment in line.adjustments:
            note = f"  {adjustment.kind}: {adjustment.reason}" if adjustment.reason else f"  {adjustment.kind}"
            out.append(f"{note:<{width - 16}}{format_amount(adjustment.amount, currency):>16}")
    out.append(rule)
    rows = [
        ("Subtotal", totals.subtotal),
        ("Discounts", totals.discounts),
        ("Surcharges", totals.surcharges),
        ("Tax", totals.tax),
        ("Credits", -totals.credits),
        ("Total due", totals.total),
    ]
    for label, value in rows:
        if value or label in {"Subtotal", "Total due"}:
            out.append(f"{label:<{width - 16}}{format_amount(value, currency):>16}")
    for warning in totals.warnings:
        out.append(f"! {warning}")
    if invoice.notes:
        out.extend(["", invoice.notes])
    return "\n".join(out) + "\n"
