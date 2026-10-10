# Fixture: tour with stop verdicts

Tests that TourStop includes an optional verdict field that can be "read" or "skim".

## Validation rules

- schema version must be 3
- tour.chapters[].stops[].verdict must be one of: "read", "skim", or absent (string, optional)
- tour.chapters[].lines must count read vs skim lines based on stop verdicts
  - stop verdict overrides part verdict
  - read verdict stops contribute to .read count
  - skim verdict stops contribute to .skim count
