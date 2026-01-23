; ModuleID = 'main'
source_filename = "main"

define double @add(double %a, double %b) {
entry:
  %b2 = alloca double, align 8
  %a1 = alloca double, align 8
  store double %a, ptr %a1, align 8
  store double %b, ptr %b2, align 8
  %a3 = load double, ptr %a1, align 8
  %b4 = load double, ptr %b2, align 8
  %addtmp = fadd double %a3, %b4
  ret double %addtmp
}

define double @quilon_main() {
entry:
  %calltmp = call double @add(double 5.000000e+00, double 7.000000e+00)
  ret double %calltmp
}
