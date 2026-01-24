~ Methods on Structs - Fully Implemented

~ Define a User type with methods
User = {
  name :: String,
  age :: Num,
  
  ~ Methods with implicit "it" parameter
  getName = => it.name,
  getAge = => it.age,
  incrementAge = amount :: Num => it.age + amount,
  isAdult = => it.age >= 18
}

~ Create a user instance
>> = () -> Num => <
  user = User { name = "Alice", age = 30 }
  
  ~ Call methods
  name = user.getName()           ~ Returns "Alice"
  age = user.getAge()             ~ Returns 30
  newAge = user.incrementAge(5)   ~ Returns 35
  adult = user.isAdult()          ~ Returns true (1)
  
  ~ Methods can be chained (if they return the right type)
  ~ message = user.getName().toUpper()  ~ Future: String methods
  
  newAge
>
