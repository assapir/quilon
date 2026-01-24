~ Test constructors and method calls

User = {
  name :: String,
  age :: Num,
  
  getName = => it.name,
  getAge = => it.age,
  incrementAge = delta :: Num => it.age + delta
}

>> = () -> Num => <
  ~ Create a user instance using constructor
  user = User { name = "Alice", age = 30 }
  
  ~ Call methods on the user
  name = user.getName()
  age = user.getAge()
  newAge = user.incrementAge(5)
  
  newAge
>
