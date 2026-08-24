~ Allocate an array per iteration and drop it — the collector's problem, not the loop's.
churn = (n :: Num, acc :: Num) -> Num => n == 0 ? acc : churn(n - 1, acc + [n, n + 1, n + 2].size)
^ = () -> Num => churn(3000000, 0) > 0 ? 0 : 1
