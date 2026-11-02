SELECT month ::XAXIS, ROUND(SUM(units*price),0) ::LINECHART, 'Monthly revenue'::TITLE FROM sales GROUP BY 1 ORDER BY 1;
