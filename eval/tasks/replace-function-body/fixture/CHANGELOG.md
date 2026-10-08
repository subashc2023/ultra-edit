# Changelog

All notable changes to this project are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `split_invoice` numbers parts `<number>-1`, `<number>-2`, and so on.

### Changed

- `render_invoice_text` accepts a `width` argument (default 72).
- Zero-decimal currencies (JPY) round the final total to whole units.

## [2.3.0] - 2026-08-14

### Changed

- `compute_totals` reports tax per tax code in `tax_by_code`.
- Credits larger than the invoice amount are capped and produce a warning.

### Fixed

- `merge_invoices` no longer drops adjustments from merged lines.

## [2.2.1] - 2026-06-30

### Fixed

- `prorate_line_item` rejects `days_used` greater than `days_in_period`.