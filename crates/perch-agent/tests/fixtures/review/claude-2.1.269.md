### 🔴 Important: Preserve the cancellation fence
**Location:** `src/lib.rs:7-9`

The new path can publish completion before its descendant process is reaped.

### 🟡 Nit: Avoid an unnecessary allocation
**Location:** `src/lib.rs:15`

This clone is not needed after the ownership change.
