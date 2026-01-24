~ Test method call syntax with type constructors

User = {
  name :: String,
  age :: Num,
  
  getName = => it.name,
  getAge = => it.age,
  incrementAge = amount :: Num => it.age + amount
}

>> = () -> Num => <
  ~ Create instance using constructor
  user = User { name = "Alice", age = 30 }
  
  ~ Method call syntax (desugared to getName(user))
  name = user.getName()
  
  ~ Method with arguments
  newAge = user.incrementAge(5)
  
  ~ Access result
  age = user.getAge()
  
  newAge
>
