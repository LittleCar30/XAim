package fixture

import ("fmt"; "strconv")
func ParseCount(input string) (int, error) {
    number, _ := strconv.Atoi(input)
    if number < 0 { return 0, fmt.Errorf("negative count") }
    return number, nil
}
