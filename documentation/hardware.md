# FLXC1000 - Hardware Documentation

## 1. Overview

The FLXC1000 demonstrator is based on a Raspberry Pi 4 running a customized embedded Linux system generated using Buildroot.

The project simulates an automotive Electronic Control Unit (ECU) and supports diagnostic communication over Ethernet using Diagnostics over Internet Protocol (DoIP) and Unified Diagnostic Services (UDS).

The demonstrator also includes visual feedback components, consisting of an OLED display and addressable RGB LEDs. These components provide visual feedback about system information, operating state and diagnostic events.

This document describes the hardware components, electrical connections, physical arrangement and troubleshooting procedures for the FLXC1000 demonstrator.


## 2. Main Components

The main hardware components used in the project are listed below.

| Component | Quantity | Description |
|-----------|----------|-------------|
|Raspberry Pi 4| 1 | Main processing unit running the embedded Linux system
|SSD1306 OLED Display| 1 |0.96-inch display using I²C communication|
|WS2812 RGB LED Sticks| 3 |Addressable RGB LED sticks with 8 LEDs each|
|Prototype Board| 1 |Mounting and electrical connection platform |
|Connecting Wires| As required| Power and signal connection |
|Power Supply| As required | Supplies regulated power to the hardware|
|Ethernet Cable| 1 | Connects the Raspberry Pi to the diagnostic host|
|microSD card| 1 |Storage for the embedded Linux system, application binaries and persistent data|

The three RGB sticks provide a total of 24 individually addressable LEDs.

The detailed Bill of Materials (BOM), including component specifications and additional assembly materials, is available in the [Supporting Documents](#6-supporting-documents) section.


## 3. Hardware Design
### 3.1 Electrical Design

The FLXC1000 hardware was designed and documented using KiCad.
The electrical schematic provides the detailed connections between the Raspberry Pi 4, OLED display, RGB LED sticks and power distribution circuitry.
It documents the relevant GPIO signals, communication interfaces, supply connections and interconnections between components.

|Signal|GPIO|Physical Pin|
|-|-|-|
|RGB data| 18 | 12|
|OLED SDA| 2 | 3 |
|OLED SCL| 3 | 5 |
|OLED VCC 3,3V| - | 1 |
|OLED GND| - | 6 |

The complete wirring diagram can be found in the [Supporting Documents](#6-supporting-documents) paragraph.

### 3.2 Mechanical Design

The mechanical enclosure was developed using FreeCAD.
The enclosure is designed to accommodate the Raspberry Pi 4, prototype board, RGB LEDs, OLED display and connecting wires.
Its purpose is to organize the components, provide mechanical protection and create a physical representation of the flux capacitor demonstrator.

The editable FreeCAD model and exported 3D files are included in the [Supporting Documents](#6-supporting-documents).

## 4. Visual Feedback

The FLXC1000 demonstrator uses the RGB LEDs and OLED display to provide visual feedback about its operating state.

### 4.1 RGB Idle Animation

During normal operation, the RGB LEDs display an idle animation representing the flux capacitor. A moving light pulse travels across the three LED branches. The animation runs while no higher priority indication is active

### 4.2 Ethernet Disconnection

When the Ethernet connection is unavailable, the RGB LEDs display a red warning indication. This allows the network connection state to be identified visually without requiring access to a terminal.

### 4.3 OLED Display

The OLED display presents system status and diagnostic information directly on the demonstrator. Depending on the firmware version, the displayed information includes:
1. Application software version.
2. Flux capacitor operating state.
3. Power status.
4. Ethernet link status.
5. Last recorder error.

Selected diagnostic operations can temporarily display additional information before the OLED returns to its normal status screen.

### 4.4 SelfTest Indication

During the execution of the SelfTest routine, the RGB LEDs provide a visual feedback indicating that the diagnostic operation is in progress. The SelfTest indication takes priority over the normal idle animation. Once the routine has completed, the LEDs return to their normal state.

## 5. Additional Hardware Notes

### 5.1 Power Distribution

The prototype board is used to organize electrical connections and distribute power to the external components.
The RGB LED sticks share power and ground connections while receiving individually addressed control data.
The electrical supply must provide the appropriate voltage and sufficient current for the connected devices.

**Important considerations:**

- The Raspberry Pi and external LED circuitry must share a common ground where required for signal communication.
- Verify supply voltage and polarity before connecting the components.
- Ensure that the power distribution can safely support the expected current.
- Inspect solder joints and connecting wires before applying power.
- Avoid short circuits and unintended contact between conductive surfaces.
- Consider signal-level compatibility between the Raspberry Pi's 3.3V GPIO outputs and the WS2812 LED data inputs.

### 5.2 Physical Assembly

The Raspberry Pi and other electronic components are arranged inside the custom-designed enclosure.
The prototype board provides a platform for soldered electrical connections, while the enclosure organizes the RGB sticks and OLED display.
During assembly, attention should be given to component alignment, wire routing, mechanical clearance and accessibility of external connections.

### 5.3 Maintenance and Modifications

When modifying the hardware, update the corresponding KiCad schematic or FreeCAD model.
The engineering files should remain the primary references for the electrical and mechanical design.
If a modification changes component quantities or specifications, the Bill of Materials should also be updated.

### 5.4 Starting the Demonstrator

Before powering on the FLXC1000, verify that the electrical connections match the KiCad schematic and that the SD card containing the system image is inserted into the Raspberry Pi.
Connect the designated power supply and allow the Raspberry Pi to complete its boot sequence.
Once the system has started, the OLED display should show status information, while the RGB LEDs indicate the current operating state.

For instructions on building and flashing the system image, refer to the main [README](../README.md).

## 6. Supporting Documents

The supporting documents are stored in:

`documentation/docs/`

This directory contains the editable engineering files and exported documents used in the development of the FLXC1000 demonstrator.

### 6.1 Bill of Materials

The Bill of Materials lists the components used in the project.

- [View BOM - Excel](docs/BOM_flxc1000.xlsx)
- [View BOM - LibreOffice](docs/BOM_flxc1000.ods)

### 6.2 Electrical Design - KiCad

The KiCAD schematic documents the electrical connections and power distribution of the demonstrator.

- [View circuit wiring diagram - KiCad](docs/Hardware_Wirring_Diagram_flxc1000-rpi.pdf)

### 6.3 Mechanical Design - FreeCAD

The mechanical enclosure was developed using FreeCAD.

The original project file can be used to modify the design, while exported formats allow viewing and manufacturing using other applications.

- [View 3D model of the physical demonstrator - FreeCAD - stl file](docs/FluxCase.stl)

- [View 3D model of the physical demonstrator - FreeCAD - FCStd file](docs/FluxCase.FCStd)

## 7. Troubleshooting

This section documents common issues encountered during development and operation of the FLXC1000 demonstrator. Troubleshooting is divided into hardware and software issue.

### 7.1 Hardware Troubleshooting

The following table provides a quick overview of common hardware-related problems.

|Problem | Possible Cause| Troubleshooting|
|--------|-|-|
|Raspberry Pi does not boot| Power supply or boot image issue| Verify power, SD card, kernel and device tree|
|RGB LEDs remain off| Incorrect power or data connection| Check supply voltage, common ground, data signal and solder joints|
|RGB animation direction is incorrect|LED orientation or indexing|Verify physical orientation and software LED ordering|
|OLED remains blank|I²C configuration or wiring issue|Check power, I²C connections and device address|
|Intermittent hardware behaviour|Unstable power or loose connections|Inspect power distribution, wiring and solder joints|

### Raspberry Pi Boot Issues

During development, the Raspberry Pi initially failed to boot because of kernel and boot configuration problems.

When investigating similar issues:

1. Verify the SD card image.
2. Confirm the appropriate Raspberry Pi 4 kernel and device tree are included.
3. Check the required boot files and configuration.
4. Verify the power supply,
5. Inspect available boot logs.

### RGB LED Issues

If the LEDs do not respond:

1. Check the regulated power supply and common ground.
2. Verify the electrical connections against the KiCad schematic.
3. Inspect the DIN/DOUT signal chain.
4. Check solder joints.
5. Review the RGB initialization logs and configuration.

If one branch displays its animation in the opposite direction, verify the physical arrangement and corresponding software indexing.

### OLED Display Issues

If the OLED remains blank:

1. Verify the electrical connections using the KiCad schematic.
2. Ensure that I²C is enabled.
3. Confirm that the display responds at address 0x3C.
4. Inspect application logs for initialization errors.

On systems with the necessary I²C utilities installed, connected devices can be inspected using:

`i2cdetect -y 1`

### 7.2 Software Troubleshooting

The following table summarizes the common software-related problems.

|Problem | Possible Cause | Troubleshooting|
|-|-|-|
|Application does not start | Missing binary, mount issue or boot state| Verify the application files, mount points and persistent state|
|Ethernet communication fails| Interface or network configuration | Check link status, IP address and connectivity|
|DoIP communication fails| Network or diagnostic configuration| Verify routing activation, logical addresses and DoIP server|
|Software update fails |Download, transfer or state error| Inspect UDS transfer, bootloader  logs and application slots|
|Candidate application fails| Invalid or crashing candidate | Check candidate binary, validation and rollback state|

### Application Startup Issues

The customized Linux system uses separate locations for application binaries and persistent data.

The application partition is mounted at `/app`, while persistent state is stored under `/data`.

The V1/V2 update mechanism uses application binaries such as:
- `/app/flxc1000-app-v1`
- `/app/flxc1000-app-v2`

If the application does not start:

1. Confirm that the relevant partitions are mounted.
2. Verify that `/app` and `/data` exist.
3. Check that the expected application binary is present and executable.
4. Inspect `/data/boot_state`.
5. Review available initialization and application logs.

A missing mount-directory issue was encountered during development and resolved by ensuring that the required directories were created before mounting the partitions.

### Ethernet Communication Issues

If the laptop cannot communicate with the Raspberry Pi, inspect the network interfaces:

`ip link`

Check assigned IP address:

`ip addr`

Verify that both devices are configured for intended network, then test the connectivity using `ping`.

During development, a link-local addressing setup was used:

|Device| IP Address|
|-|-|
|Laptop|`169.254.16.2/16`|
|Raspberry Pi|`169.254.16.1/16`|

These addresses represent the development configuration and may differ in other environments.

An Ethernet interface reporting `DOWN` or `NO-CARRIER` should be investigated before attempting higher-level DoIP debugging.

**DoIP and UDS Communication Issues**

If Ethernet connectivity works but diagnostic requests fail:

1. Confirm the DoIP server is running.
2. Verify the routing activation.
3. Check the source and target logical addresses.
4. Inspect the UDS request and response.
5. Review diagnostic logs.


## 8. Related Documentation

For additional information about the FLXC1000 architecture, Rust compilation, Buildroot setup, diagnostic services and software functionality, refer to the repository's main [README](../README.md).

The wirring diagram, bill of materials and enclosure design files can be found in the [Supporting Documents](#6-supporting-documents) section.
