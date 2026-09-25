package main

import (
	"fmt"
	"os"
)

func main() {
	port := os.Getenv("PORT")
	if port == "" {
		port = "8080"
	}
	fmt.Println("listening on :" + port)
}
