# `.xlsx` Parser Library Requirements Specification

*[Japanese](requirements.md)*

## 0. Implementation Language

Rust

## 1. Project Overview

Develop a lightweight, high-performance `.xlsx` (OOXML) parser library as a replacement for existing `.xlsx` parsers. The goal is to process and analyze — without loading a full in-memory grid — the shapes that come up constantly in Japanese business systems: "grid-paper Excel" (an extreme number of rows/columns, sometimes reaching millions of cells) and heavy use of merged cells, and to return the result as a JSON format that's easy to consume from a frontend or another system.

## 2. System Architecture and Processing Pipeline

The parser operates as a one-directional data processing pipeline made up of the following 5 phases.

### Phase 1: Relationship Resolution and Resource Disposal (Expand → Discard)

* **Processing:** Expand the `_rels` files (e.g. `xl/_rels/workbook.xml.rels`) from the ZIP archive and build, in memory, a routing map linking each sheet ID (`r:id`) to its part's file path (e.g. `worksheets/sheet1.xml`).
* **Mandatory requirement:** To improve memory efficiency and prevent temporary data from lingering, **the expanded `_rels` scratch data and any related resources must be deleted/discarded from memory (or the filesystem) immediately once the routing map has been built**.

### Phase 2: Sanitization (Rejecting Malicious Injection)

* **Processing:** Assume the incoming file's safety is not guaranteed, and interpose the following security mechanisms as a layer before any content is trusted.
* **Requirements:**
  * **Zip countermeasures:** Detect and block Zip Bombs (memory-exhaustion attacks via highly compressed files) and path traversal (Zip Slip) during decompression.
  * **XXE countermeasures:** Disable external entity expansion during XML parsing, to prevent unauthorized local file reference.

### Phase 3: Streaming Parse and Boundary Definition (Paging)

* **Processing:** To prevent memory exhaustion, never expand the target sheet's (`sheetX.xml`) full DOM into memory.
* **Requirements:** Use an event-driven (SAX-style) parser, treating each `<row>` inside `<sheetData>` as a processing boundary. Once one row's worth of data has been read and retained (as described below), discard that row's XML nodes.

### Phase 4: Analysis and Deferred Resolution

* **Processing:** Convert and combine the collected raw data into a meaningful data structure.
* **Requirements:**
  * **Shared string / style resolution:** When a cell's value is `t="s"` (a shared-string index), look it up against the retained `SharedStringTable` and assign the actual string data.
  * **Deferred merged-cell resolution:** After the streaming parse completes, read the `<mergeCells>` element that appears near the end of the sheet. Cross-reference the list of merged ranges (e.g. `A1:C3`) against the cell data already collected to establish merge state.

### Phase 5: JSON Generation (Return)

* **Processing:** Serialize the fully analyzed and resolved data model into JSON.
* **Requirements:** To make rendering easy on a frontend, return structured JSON with attributes such as `row_span`/`col_span` attached.

## 3. Core Functional Requirements (Business-System-Specific Requirements)

### 3.1 Memory Optimization via Sparse Matrix (Grid-Paper Excel Countermeasure)

* Allocating data as a 2D array (`rows x columns`) is prohibited.
* Only cells that hold data or formatting are kept, in a hash map (e.g. `HashMap`) keyed by coordinate (e.g. `row: 1, col: 1`).
* Blank cells never get an in-memory instance, and are also excluded from JSON output (or, where necessary, emitted as a minimal `null`).

### 3.2 Transparent Access Support for Merged Cells

* For a merged cell (e.g. `A1:C3`), account for access not only to the origin cell holding the real data (`A1`), but also to the virtual cell coordinates contained within the merged range (`B1`, `C2`, etc.).
* During the analysis phase, internally map an "alias reference" from each virtual cell to the origin cell (`A1`), so that whichever coordinate is requested, the correct merged value and merge metadata are returned.

## 4. Primary OOXML Spec Files Handled

* `[Content_Types].xml`: MIME type definitions for each part
* `xl/workbook.xml`: sheet composition definitions
* `xl/sharedStrings.xml`: centralized string data (must honor `xml:space="preserve"`)
* `xl/styles.xml`: cell formatting/style definitions
* `xl/worksheets/sheetX.xml`: a sheet's actual data (includes `<sheetData>`, `<mergeCells>`, etc.)
