~ Test command-line arguments
~ Entry point that receives argc and argv
>> = (argc :: Num, argv :: Num) -> Num => <
  ~ argc is the number of arguments (including program name)
  ~ argv is currently just a placeholder (0) - proper array conversion TODO
  
  ~ Example:
  ~ ./args_test           => exit 1 (just program name)
  ~ ./args_test a b c     => exit 4 (program + 3 args)
  
  argc
>
