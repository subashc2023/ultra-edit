# billing

Invoice arithmetic and the HTTP API for the billing service.

```python
from billing import compute_totals, invoice_from_dict

totals = compute_totals(invoice_from_dict(payload))
print(totals.total)
```

`compute_totals` never mutates the invoice. Amounts are `Decimal` values in the
invoice currency, rounded to the cent.

## Development

```sh
python -m pip install -e '.[test]'
python -m pytest
```
