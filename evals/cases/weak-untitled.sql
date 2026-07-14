-- request: "Show the numbers by category." (a deliberately mediocre answer)
CREATE TABLE t AS SELECT * FROM (VALUES ('a',10),('b',20),('c',15),('d',30)) v(k,n);
SELECT k ::XAXIS, n ::BARCHART FROM t;
SELECT k ::XAXIS, n ::LINECHART FROM t;
