~ Test logical operators (&&, ||)
~ Entry point that tests boolean logic
>> = () -> Num => <
  ~ Test AND operator
  a = true && true      ~ Should be true (1)
  b = true && false     ~ Should be false (0)
  c = false && true     ~ Should be false (0)
  
  ~ Test OR operator
  d = true || false     ~ Should be true (1)
  e = false || false    ~ Should be false (0)
  f = false || true     ~ Should be true (1)
  
  ~ Test complex expression
  ~ (true && false) || (true && true) => false || true => true
  complex = (true && false) || (true && true)
  
  ~ Return 1 if complex is true, 0 otherwise
  complex ? 1 : 0
>
