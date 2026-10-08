Add a configurable rounding mode to invoice totals. Three files change: `billing/invoice.py`, `billing/api.py`, and `CHANGELOG.md`. Every other file, including `billing/store.py`, `billing/tax.py`, `billing/__init__.py`, and `tests/test_invoice.py`, stays exactly as it is.

All Python code uses spaces for indentation, never tabs. Each code block below is reproduced exactly, with its full indentation; insert it as shown.

## 1. `billing/invoice.py`

**1a. Signature.** Replace the single line

```python
def compute_totals(invoice: Invoice, *, tax_table: TaxTable | None = None) -> InvoiceTotals:
```

with these three lines:

```python
def compute_totals(
    invoice: Invoice, *, tax_table: TaxTable | None = None, rounding: str = "half_up"
) -> InvoiceTotals:
```

**1b. Docstring.** Replace the five-line docstring of `compute_totals`

```python
    """Return the subtotal, discounts, tax, credits, and total of ``invoice``.

    Tax is computed per line and rounded half-up to the cent, then summed.
    Credits are applied last and never take the total below zero.
    """
```

with this six-line docstring (indented 4 spaces; the second line is empty):

```python
    """Return the subtotal, adjustments, tax, credits, and total of ``invoice``.

    Taxable net amounts are summed per tax code and each code's tax is rounded
    once, using ``rounding`` ("half_up" or "half_even"). Credits are applied
    last and never take the total below zero.
    """
```

**1c. Body.** In `compute_totals`, replace every line of the body after the docstring, from the line `    table = tax_table or default_tax_table()` through the first following line that is exactly `    return totals` (4 spaces of indentation), inclusive. That is 32 lines. Replace them with these 40 lines (4-space indentation per level; the braces, quotes, and `!r` in the f-strings are literal):

```python
    if rounding not in ROUNDING_MODES:
        raise InvoiceError(
            f"unknown rounding mode {rounding!r}; expected one of {{'half_up', 'half_even'}}"
        )
    mode = ROUNDING_MODES[rounding]
    table = tax_table or default_tax_table()
    totals = InvoiceTotals()
    taxable_net: dict[str, Decimal] = defaultdict(lambda: ZERO)
    for line in invoice.lines:
        totals.subtotal += line.gross
        for adjustment in line.adjustments:
            if adjustment.is_discount:
                totals.discounts += adjustment.amount
            else:
                totals.surcharges += adjustment.amount
        if not line.tax_exempt:
            taxable_net[line.tax_code] += line.net
    for code in sorted(taxable_net):
        code_tax = _quantize(taxable_net[code] * table.rate_for(code), mode)
        totals.tax_by_code[code] = code_tax
        totals.tax += code_tax
    exempt_skus = [line.sku for line in invoice.lines if line.tax_exempt]
    if exempt_skus:
        totals.warnings.append(f"tax-exempt lines: {', '.join(exempt_skus)}")
    totals.subtotal = _quantize(totals.subtotal, mode)
    totals.discounts = _quantize(totals.discounts, mode)
    totals.surcharges = _quantize(totals.surcharges, mode)
    before_credits = totals.subtotal + totals.discounts + totals.surcharges + totals.tax
    available = sum(invoice.credits, ZERO)
    if available > before_credits:
        totals.warnings.append(
            f"credits of {available} exceed the invoice amount of {before_credits}; "
            "the excess was not applied"
        )
        available = before_credits
    totals.credits = _quantize(available, mode)
    totals.total = _quantize(before_credits - totals.credits, mode)
    if invoice.currency in ZERO_DECIMAL_CURRENCIES:
        totals.total = totals.total.quantize(Decimal("1"), rounding=mode)
    return totals
```

The next function, `summarize_by_customer`, also ends with `    return totals`; leave it unchanged. The blank lines between `compute_totals` and `summarize_by_customer` stay as they are.

**1d. One call site.** In `prorate_line_item`, replace the line

```python
    prorated = _round_cents(line.unit_price * Decimal(days_used) / Decimal(days_in_period))
```

with

```python
    prorated = _quantize(line.unit_price * Decimal(days_used) / Decimal(days_in_period))
```

Only this call changes. The `_round_cents` calls inside the old `compute_totals` body disappear with step 1c; every other `_round_cents` in the file (the helper definition and the calls in `summarize_by_customer`, `apply_credit`, `aging_buckets`, and `format_amount`) stays as it is.

## 2. `billing/api.py`

**2a.** In `get_invoice`, replace the line

```python
    totals = compute_totals(invoice, tax_table=tax_table)
```

with

```python
    totals = compute_totals(invoice, tax_table=tax_table, rounding="half_even")
```

**2b.** In `create_invoice`, replace these four lines

```python
        totals = compute_totals(
            invoice,
            tax_table=resolve_tax_table(request.region),
        )
```

with these five lines (the new line is indented 12 spaces and ends with a comma):

```python
        totals = compute_totals(
            invoice,
            tax_table=resolve_tax_table(request.region),
            rounding="half_even",
        )
```

Leave `compute_totals(draft)` in `preview_invoice` unchanged.

## 3. `CHANGELOG.md`

Under `## [Unreleased]`, the `### Changed` subheading is followed by one blank line and then the entry

```markdown
- `render_invoice_text` accepts a `width` argument (default 72).
```

Insert this new line directly before that entry, so it becomes the first entry under that `### Changed` (the blank line after the subheading stays, and nothing else moves):

```markdown
- `compute_totals` takes a `rounding` argument (`"half_up"` or `"half_even"`); the API uses `"half_even"`.
```

The `### Changed` subheading under `## [2.3.0] - 2026-08-14` does not change. `CHANGELOG.md` has no newline at the end of its last line; keep it that way.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
