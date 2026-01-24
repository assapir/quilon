~ Test method call syntax

~ Define methods as functions with self parameter  
getName = self => self.name
getAge = self => self.age
incrementAge = (self, amount) => self.age + amount

~ Test method calls
>> = () -> Num => <
  user = { name = "Alice", age = 30 }
  
  ~ Method call syntax (desugared to getName(user))
  name = user.getName()
  
  ~ Method with arguments
  newAge = user.incrementAge(5)
  print(newAge)
  
  ~ Regular function call syntax still works
  age = getAge(user)
  print(age)
  
  0
>
