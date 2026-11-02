-- #845 (M3-3): the supplier lead-time data (web/data/supplier-lead-times/):
-- purchase order lines, their receipts and the suppliers.
CREATE TABLE po_history AS FROM read_csv('data/supplier-lead-times/po_history.csv');
CREATE TABLE po_receipt AS FROM read_csv('data/supplier-lead-times/po_receipt.csv');
CREATE TABLE suppliers AS FROM read_csv('data/supplier-lead-times/suppliers.csv');
