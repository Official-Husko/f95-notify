package main

import (
	"embed"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"os/exec"
	"time"

	"github.com/gen2brain/beeep"
	"github.com/getlantern/systray"
)

//go:embed icon.ico
var content embed.FS

// Config represents the configuration structure
type Config struct {
	XFToken string `json:"_xfToken"`
	Cookie  string `json:"cookie"`
}

// Define a struct to represent the JSON data
type Data struct {
	Visitor struct {
		TotalUnread json.Number `json:"total_unread"`
	} `json:"visitor"`
}

// Define a constant for the version
const version = "1.0.0" // Replace with your actual version

// main is the entry point function for the program.
//
// It opens a log file for writing, sets the log output to both the file and standard output,
// checks if a config file exists and creates it if not,
// reads the configuration from the config file,
// and initializes the system tray.
func main() {
	// Open the log file for writing
	logFile, err := os.Create("runtime.log")
	if err != nil {
		// If there's an error opening the log file, log it and continue
		log.Println("Error opening log file:", err)
	}

	// Set the log output to both the file and standard output
	log.SetOutput(io.MultiWriter(os.Stdout, logFile))

	// Close the log file when you're done
	logFile.Close()

	// Check if config file exists, create it if not
	configPath := "config.json"
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		if err := createDefaultConfig(configPath); err != nil {
			log.Fatalf("Error creating default config: %v\n", err)
		}
	}

	// Read the configuration
	config, err := readConfig(configPath)
	if err != nil {
		log.Fatalf("Error reading config: %v\n", err)
	}

	// If values are empty, close the program
	if config.XFToken == "" || config.Cookie == "" {
		log.Fatal("XFToken and Cookie values are required. Closing the program.")
	}

	// Initialize the system tray
	systray.Run(onReady(config), onExit)
}

// onReady sets up the system tray icon and menu.
//
// It takes a `config` parameter of type `Config` and returns a function that
// sets up the system tray icon and menu.
func onReady(config Config) func() {
	return func() {
		// Read the icon file
		iconData, err := content.ReadFile("icon.ico")
		if err != nil {
			log.Printf("Error reading embedded icon file: %v\n", err)
			return
		}

		// Set up the system tray icon and tooltip
		systray.SetIcon(iconData)
		systray.SetTooltip("F95-Notify v" + version)

		// Add menu items
		mViewNotifications := systray.AddMenuItem("View Notifications", "View unread notifications")
		mVisitGithub := systray.AddMenuItem("Visit Github", "Visit the GitHub repository")
		mQuit := systray.AddMenuItem("Quit", "Quit the application")

		// Run the background process in a goroutine
		go runBackgroundProcess(config)

		// Handle menu item clicks
		go func() {
			for {
				select {
				case <-mViewNotifications.ClickedCh:
					openURL("https://f95zone.to/account/alerts") // Replace with your actual URL
				case <-mVisitGithub.ClickedCh:
					openURL("https://github.com/Official-Husko/f95-notify") // Replace with your actual GitHub URL
				case <-mQuit.ClickedCh:
					log.Println("Quit menu item clicked")
					systray.Quit()
				}
			}
		}()
	}
}

// openURL opens the specified URL in the default web browser.
//
// It takes a string parameter `url` which represents the URL to be opened.
// The function does not return anything.
func openURL(url string) {
	// Command represents an external command being prepared or run.
	cmd := exec.Command("rundll32", "url.dll,FileProtocolHandler", url)

	// Start starts the specified command but does not wait for it to complete.
	err := cmd.Start()
	if err != nil {
		// Printf formats its arguments according to the format, analogous to C's printf.
		log.Printf("Error opening URL: %v\n", err)
	}
}

// onExit is a function that performs cleanup tasks before exiting the program.
//
// It does not take any parameters.
// It does not return any values.
func onExit() {
	// Print a log message indicating that cleanup is being performed
	log.Println("Cleaning up...")

	// Print a message indicating that the program is exiting
	fmt.Println("Exiting...")

	// Terminate the program with exit code 0
	os.Exit(0)
}

// runBackgroundProcess runs a background process that periodically makes a GET request to a specified URL
// and processes the response.
//
// It takes a Config struct as a parameter which contains the necessary configuration values such as the
// URL, headers, and authentication tokens.
//
// The function does not return anything.
func runBackgroundProcess(config Config) {
	// set up a ticker to trigger the function periodically
	ticker := time.NewTicker(5 * time.Minute)
	defer ticker.Stop() // stop the ticker when the function ends

	for range ticker.C { // iterate over the ticker channel
		// specify the URL for the GET request
		url := "https://f95zone.to/account/unread-alert?_xfResponseType=json"

		// set up the headers for the GET request
		headers := map[string]string{
			"User-Agent": fmt.Sprintf("F95 Notify/%s (by Official Husko on GitHub)", version),
			"Accept":     "application/json",
			"Cookie":     config.Cookie,
			"_xfToken":   config.XFToken,
		}

		// make the GET request
		response, err := makeGetRequest(url, headers)
		if err != nil {
			log.Printf("Error making GET request: %v\n", err) // log the error and continue to the next iteration
			continue
		}

		log.Printf("Response: %s\n", string(response)) // log the response for further analysis

		var data Data                         // create a variable to hold the unmarshalled response
		err = json.Unmarshal(response, &data) // unmarshal the response into the data variable
		if err != nil {
			log.Printf("Error unmarshalling JSON: %v\n", err) // log the error and continue to the next iteration
			continue
		}

		totalUnread, err := data.Visitor.TotalUnread.Int64()
		if err != nil {
			log.Println("Error converting TotalUnread to int64:", err)
		} else if totalUnread > 0 { // check if totalUnread is greater than 0
			log.Printf("Total Unread: %d\n", totalUnread)

			title := "New Notifications!"
			body := fmt.Sprintf("You have %d unread notifications.", totalUnread)
			sendNotification(title, body)
		}
	}
}

// sendNotification sends a notification with the given title and message.
//
// Parameters:
// - title: The title of the notification.
// - message: The message content of the notification.
// Return type: None.
func sendNotification(title, message string) {
	// Use the beeep.Notify function to send a notification with the given title, message, and icon.
	err := beeep.Notify(title, message, "icon.ico")

	// If there is an error while sending the notification, log the error.
	if err != nil {
		log.Printf("Error sending notification: %v\n", err)
	}
}

// makeGetRequest sends a GET request to the specified URL with the given headers and returns the response body as a byte array.
//
// Parameters:
// - url: The URL to send the GET request to.
// - headers: A map of headers to include in the request.
//
// Returns:
// - []byte: The response body as a byte array.
// - error: An error if there was a problem sending the request or reading the response body.
func makeGetRequest(url string, headers map[string]string) ([]byte, error) {
	// Create a new HTTP client
	client := &http.Client{}

	// Create a new HTTP GET request
	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		log.Printf("Error creating request: %s", err)
		return nil, err
	}

	// Set headers
	for key, value := range headers {
		req.Header.Set(key, value)
	}

	// Send the HTTP request
	resp, err := client.Do(req)
	if err != nil {
		log.Printf("Error sending request: %s", err)
		return nil, err
	}
	defer resp.Body.Close()

	// Read the response body using io.ReadAll
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		log.Printf("Error reading response body: %s", err)
		return nil, err
	}

	// Return the response body
	return body, nil
}

// createDefaultConfig creates a default configuration and writes it to a file.
//
// Parameters:
// - filePath: the path to the file where the default configuration will be written.
//
// Returns:
// - error: if there was an error creating or writing the default configuration.
func createDefaultConfig(filePath string) error {
	// Define the default configuration
	defaultConfig := Config{
		XFToken: "Your XF Token",
		Cookie:  "Your Cookie",
	}

	// Convert the configuration to JSON
	configJSON, err := json.MarshalIndent(defaultConfig, "", "  ")
	if err != nil {
		log.Printf("Error marshaling default configuration to JSON: %s", err)
		return err
	}

	// Write the default configuration to the file
	err = os.WriteFile(filePath, configJSON, 0644)
	if err != nil {
		log.Printf("Error writing default configuration to file: %s", err)
		return err
	}

	// Print success message
	fmt.Println("Default config created successfully.")

	// Explicitly exit the program
	os.Exit(0)

	return nil
}

// readConfig reads the configuration file and returns a Config struct and an error.
//
// It takes a filePath string as a parameter, which represents the path of the configuration file.
// It returns a Config struct, which contains the configuration data, and an error if any error occurs while reading or unmarshaling the configuration file.
func readConfig(filePath string) (Config, error) {
	// Read the configuration file
	configJSON, err := os.ReadFile(filePath)
	if err != nil {
		log.Printf("Failed to read configuration file: %v", err)
		return Config{}, err
	}

	// Unmarshal the configuration
	var config Config
	err = json.Unmarshal(configJSON, &config)
	if err != nil {
		log.Printf("Failed to unmarshal configuration: %v", err)
		return Config{}, err
	}

	// Check if XFToken or Cookie is equal to default values
	if config.XFToken == "Your XF Token" || config.Cookie == "Your Cookie" {
		log.Println("Config values are set to default. Exiting.")
		os.Exit(0)
	}

	return config, nil
}
