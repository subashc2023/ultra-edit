import datetime as dt
from decimal import Decimal

import pytest

from billing.invoice import (
    Adjustment,
    Invoice,
    InvoiceError,
    LineItem,
    compute_totals,
    prorate_line_item,
    split_invoice,
)
from billing.tax import default_tax_table


def make_invoice(*lines, credits=(), currency="USD"):
    return Invoice(
        number="INV-2026-00001",
        customer_id="acme",
        currency=currency,
        issued_on=dt.date(2026, 9, 1),
        due_on=dt.date(2026, 10, 1),
        lines=tuple(lines),
        credits=tuple(Decimal(credit) for credit in credits),
    )


def test_compute_totals_applies_standard_tax():
    invoice = make_invoice(LineItem("widget", "Widget", 3, Decimal("10.00")))
    totals = compute_totals(invoice, tax_table=default_tax_table())
    assert totals.subtotal == Decimal("30.00")
    assert totals.tax == Decimal("6.00")
    assert totals.total == Decimal("36.00")


def test_compute_totals_separates_discounts_and_surcharges():
    line = LineItem(
        "widget",
        "Widget",
        1,
        Decimal("100.00"),
        adjustments=(Adjustment("discount", Decimal("-10.00")), Adjustment("rush", Decimal("5.00"))),
    )
    totals = compute_totals(make_invoice(line))
    assert totals.discounts == Decimal("-10.00")
    assert totals.surcharges == Decimal("5.00")


def test_compute_totals_caps_credits():
    invoice = make_invoice(LineItem("widget", "Widget", 1, Decimal("10.00")), credits=["50"])
    totals = compute_totals(invoice)
    assert totals.total == Decimal("0.00")
    assert totals.warnings


def test_prorate_line_item_rounds_to_cents():
    line = LineItem("seat", "Seat", 1, Decimal("30.00"))
    assert prorate_line_item(line, days_used=10, days_in_period=31).unit_price == Decimal("9.68")


def test_split_invoice_keeps_credits_on_first_part():
    lines = [LineItem(f"sku-{n}", "", 1, Decimal("1")) for n in range(5)]
    parts = split_invoice(make_invoice(*lines, credits=["2"]), max_lines=2)
    assert [part.number for part in parts] == ["INV-2026-00001-1", "INV-2026-00001-2", "INV-2026-00001-3"]
    assert parts[1].credits == ()


def test_rejects_unknown_currency():
    with pytest.raises(InvoiceError):
        make_invoice(currency="XYZ")
