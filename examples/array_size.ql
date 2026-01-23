~ Test array .size field
~ Arrays are now structs with { ptr data, i64 size }
>> = () -> Num => <
  nums = [1, 2, 3, 4, 5]
  
  ~ Access the size field
  count = nums.size
  
  ~ Should return 5
  count
>
