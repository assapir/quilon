~ Test method calls

User = {
  name :: String,
  age :: Num,
  
  getName = => it.name,
  getAge = => it.age,
  incrementAge = delta :: Num => it.age + delta
}

>> = () -> Num => <
  ~ Methods are type-checked properly
  ~ Once we have constructors/type ascription, we can actually call them
  
  0
>
