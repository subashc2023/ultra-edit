package main

import (
	"fmt"
	"os"
)

func main() {
	port := os.Getenv("PORT")
	if port == "" {
		port = "9090"
		fmt.Fprintln(os.Stderr, "PORT not set; using 9090")
	}
	fmt.Println("listening on :" + port)
}
