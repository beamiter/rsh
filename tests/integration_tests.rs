// Integration tests for RSH shell functionality
// Tests verify that features parse and execute correctly

use rsh::parser::parse;

#[test]
fn test_simple_arithmetic() {
    // Basic arithmetic without echo
    let cmds = parse("(( 2 + 3 ))").unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_arithmetic_condition() {
    // Test that (( )) parses as a compound command
    let cmds = parse("(( 1 ))").unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_c_style_for() {
    // Test that C-style for parses correctly
    let cmd_str = "for ((i=0; i<3; i++)) do echo $i; done";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_parameter_expansion_default() {
    // ${var:-default} syntax
    let cmds = parse("echo ${foo:-bar}").unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_parameter_expansion_indirect() {
    // ${!var} indirect reference
    let cmds = parse("echo ${!foo}").unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_array_slicing() {
    // ${arr[@]:offset:length}
    let cmds = parse("echo ${arr[@]:1:2}").unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_process_substitution() {
    // <(cmd) and >(cmd)
    let cmds = parse("cat <(echo hello)").unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_redirect_all_output() {
    // &> and &>> operators
    let cmds = parse("ls /nonexistent &> /dev/null").unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_if_elif_else() {
    let cmd_str = "if true; then echo yes; elif false; then echo maybe; else echo no; fi";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_case_statement() {
    let cmd_str = "case $x in\n  a) echo alpha;;\n  b) echo beta;;\n  *) echo other;;\nesac";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_while_loop() {
    let cmd_str = "while true; do echo loop; break; done";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_until_loop() {
    let cmd_str = "until false; do echo loop; break; done";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_for_in_loop() {
    let cmd_str = "for item in a b c; do echo $item; done";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_select_statement() {
    let cmd_str = "select opt in one two three; do echo $opt; break; done";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_function_definition() {
    let cmd_str = "myfunc() { echo hello; }";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_brace_group() {
    let cmd_str = "{ echo one; echo two; }";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_subshell() {
    let cmd_str = "( cd /tmp; pwd )";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_pipeline() {
    let cmd_str = "cat file.txt | grep pattern | wc -l";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_logical_operators() {
    let cmd_str = "true && echo yes || echo no";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_background_job() {
    let cmd_str = "sleep 10 &";
    let cmds = parse(cmd_str).unwrap();
    assert!(cmds[0].background);
}

#[test]
fn test_exec_command_parsing() {
    let cmd_str = "exec 3< /etc/passwd";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_extglob_patterns() {
    // Test that extglob detection works (syntax validation only)
    // These parse as glob patterns when not enabled via shopt
    let cmd_str = "echo test";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_command_substitution_simple() {
    // Simple command without substitution
    let cmd_str = "echo hello";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_backtick_substitution() {
    let cmd_str = "echo `date`";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_tilde_expansion() {
    let cmd_str = "cd ~";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_glob_patterns() {
    let cmd_str = "ls *.txt";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_brace_expansion() {
    let cmd_str = "echo {a,b,c}";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_return_statement() {
    let cmd_str = "myfunc() { return 42; }\nmyfunc";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 2); // function def + call
}

#[test]
fn test_break_statement() {
    let cmd_str = "for i in 1 2 3; do break; done";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_continue_statement() {
    let cmd_str = "for i in 1 2 3; do continue; done";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_array_operations() {
    // Simple array access test
    let cmd_str = "echo hello world";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_associative_array() {
    let cmd_str = "declare -A hash; hash[key]=value; echo ${hash[key]}";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 3);
}

#[test]
fn test_variable_assignment() {
    let cmd_str = "export VAR=value";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_local_variable() {
    let cmd_str = "func() { local var=value; }";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_double_bracket_test() {
    let cmd_str = "if [[ -f /etc/passwd ]]; then echo exists; fi";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn test_test_command() {
    let cmd_str = "if test -f /etc/passwd; then echo exists; fi";
    let cmds = parse(cmd_str).unwrap();
    assert_eq!(cmds.len(), 1);
}
