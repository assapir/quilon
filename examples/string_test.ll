; ModuleID = 'main'
source_filename = "main"

@str = private unnamed_addr constant [6 x i8] c"World\00", align 1

define ptr @greet(ptr %name) {
entry:
  %name1 = alloca ptr, align 8
  store ptr %name, ptr %name1, align 8
  %name2 = load ptr, ptr %name1, align 8
  ret ptr %name2
}

define ptr @quilon_main() {
entry:
  %calltmp = call ptr @greet(ptr @str)
  ret ptr %calltmp
}
