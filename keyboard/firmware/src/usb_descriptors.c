#include "tusb.h"
#include "fw_protocol.h"
#include "pico/unique_id.h"

#include <string.h>

#define ITF_NUM_HID        0
#define ITF_NUM_VENDOR     1
#define ITF_NUM_CDC        2
#define ITF_NUM_CDC_DATA   3
#define ITF_NUM_HID_VENDOR 4
#define ITF_NUM_TOTAL      5

#define EPNUM_VENDOR_OUT 0x01
#define EPNUM_CDC_OUT    0x02
#define EPNUM_CDC_IN     0x82
#define EPNUM_CDC_NOTIF  0x81
#define EPNUM_HID        0x83
#define EPNUM_VENDOR_IN  0x84
#define EPNUM_HID_VENDOR 0x85

#define REPORT_ID_KEYBOARD 1
#define REPORT_ID_CONSUMER 2

// clang-format off
static const uint8_t desc_hid_kbd_report[] = {
    /* Report ID 1: Full-NKRO keyboard (1 reserved byte + 256-bit usage bitmap 0x00..0xFF + 5-bit LED output).
     * Boot-protocol hosts (BIOS/UEFI) use SET_PROTOCOL(0) and receive standard 8-byte boot keyboard reports. */
    HID_USAGE_PAGE     ( HID_USAGE_PAGE_DESKTOP                 ),
    HID_USAGE          ( HID_USAGE_DESKTOP_KEYBOARD             ),
    HID_COLLECTION     ( HID_COLLECTION_APPLICATION             ),
        HID_REPORT_ID  ( REPORT_ID_KEYBOARD                     )
        HID_REPORT_COUNT ( 1                                    ),
        HID_REPORT_SIZE  ( 8                                    ),
        HID_INPUT        ( HID_CONSTANT                         ),
        HID_USAGE_PAGE   ( HID_USAGE_PAGE_KEYBOARD              ),
        HID_USAGE_MIN    ( 0x00                                 ),
        HID_USAGE_MAX_N  ( 0x00FF, 2                            ),
        HID_LOGICAL_MIN  ( 0                                    ),
        HID_LOGICAL_MAX  ( 1                                    ),
        HID_REPORT_COUNT_N ( 256, 2                             ),
        HID_REPORT_SIZE  ( 1                                    ),
        HID_INPUT        ( HID_DATA | HID_VARIABLE | HID_ABSOLUTE ),
        HID_USAGE_PAGE   ( HID_USAGE_PAGE_LED                   ),
        HID_USAGE_MIN    ( 1                                    ),
        HID_USAGE_MAX    ( 5                                    ),
        HID_REPORT_COUNT ( 5                                    ),
        HID_REPORT_SIZE  ( 1                                    ),
        HID_OUTPUT       ( HID_DATA | HID_VARIABLE | HID_ABSOLUTE ),
        HID_REPORT_COUNT ( 1                                    ),
        HID_REPORT_SIZE  ( 3                                    ),
        HID_OUTPUT       ( HID_CONSTANT                         ),
    HID_COLLECTION_END,
    TUD_HID_REPORT_DESC_CONSUMER(HID_REPORT_ID(REPORT_ID_CONSUMER)),
};

/* Dedicated vendor HID interface (ITF_NUM_HID_VENDOR) so host software can open it without triggering
 * OS keyboard Input Monitoring permission prompts. Sole collection on this interface: no Report ID byte. */
static const uint8_t desc_hid_of_report[] = {
    HID_USAGE_PAGE_N ( 0xFF00, 2                                ),
    HID_USAGE        ( 0x01                                     ),
    HID_COLLECTION   ( HID_COLLECTION_APPLICATION               ),
        HID_USAGE_PAGE_N ( 0xFF00, 2                            ),
        HID_USAGE_MIN    ( 0x00                                 ),
        HID_USAGE_MAX    ( 0x1F                                 ),
        HID_LOGICAL_MIN  ( 0                                    ),
        HID_LOGICAL_MAX  ( 1                                    ),
        HID_REPORT_SIZE  ( 1                                    ),
        HID_REPORT_COUNT ( 32                                   ),
        HID_INPUT        ( HID_DATA | HID_VARIABLE | HID_ABSOLUTE ),
    HID_COLLECTION_END,
};
// clang-format on

const uint8_t *tud_hid_descriptor_report_cb(uint8_t instance) {
    return (instance == 1U) ? desc_hid_of_report : desc_hid_kbd_report;
}

static const tusb_desc_device_t desc_device = {
    .bLength = sizeof(tusb_desc_device_t),
    .bDescriptorType = TUSB_DESC_DEVICE,
    .bcdUSB = 0x0210,
    .bDeviceClass = TUSB_CLASS_MISC,
    .bDeviceSubClass = MISC_SUBCLASS_COMMON,
    .bDeviceProtocol = MISC_PROTOCOL_IAD,
    .bMaxPacketSize0 = CFG_TUD_ENDPOINT0_SIZE,
    .idVendor = FW_USB_VID,
    .idProduct = FW_USB_PID,
    .bcdDevice = 0x0101,
    .iManufacturer = 0x01,
    .iProduct = 0x02,
    .iSerialNumber = 0x03,
    .bNumConfigurations = 0x01,
};

const uint8_t *tud_descriptor_device_cb(void) {
    return (const uint8_t *)&desc_device;
}

/* Microsoft OS 2.0 Descriptors so Windows automatically binds WinUSB to ITF_NUM_VENDOR (MI_01). */
#define VENDOR_REQUEST_MICROSOFT 0x02
#define BOS_TOTAL_LEN            (TUD_BOS_DESC_LEN + TUD_BOS_MICROSOFT_OS_DESC_LEN)
#define MS_OS_20_DESC_LEN        0x00B2

// clang-format off
static const uint8_t desc_bos[] = {
    TUD_BOS_DESCRIPTOR(BOS_TOTAL_LEN, 1),
    TUD_BOS_MS_OS_20_DESCRIPTOR(MS_OS_20_DESC_LEN, VENDOR_REQUEST_MICROSOFT),
};

static const uint8_t desc_ms_os_20[] = {
    /* Set header: length, type, Windows version (8.1+), total length */
    U16_TO_U8S_LE(0x000A), U16_TO_U8S_LE(0x0000), U32_TO_U8S_LE(0x06030000), U16_TO_U8S_LE(MS_OS_20_DESC_LEN),

    /* Configuration subset header: length, type, configuration index (0), reserved, subset length */
    U16_TO_U8S_LE(0x0008), U16_TO_U8S_LE(0x0001), 0x00, 0x00, U16_TO_U8S_LE(MS_OS_20_DESC_LEN - 0x000A),

    /* Function subset header: length, type, first interface (ITF_NUM_VENDOR), reserved, subset length */
    U16_TO_U8S_LE(0x0008), U16_TO_U8S_LE(0x0002), ITF_NUM_VENDOR, 0x00, U16_TO_U8S_LE(MS_OS_20_DESC_LEN - 0x000A - 0x0008),

    /* MS OS 2.0 Compatible ID descriptor: length (20), type (3), compatible ID ("WINUSB"), sub-compatible ID */
    U16_TO_U8S_LE(0x0014), U16_TO_U8S_LE(0x0003),
    'W', 'I', 'N', 'U', 'S', 'B', 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,

    /* MS OS 2.0 Registry property descriptor: DeviceInterfaceGUIDs (REG_MULTI_SZ) */
    U16_TO_U8S_LE(MS_OS_20_DESC_LEN - 0x000A - 0x0008 - 0x0008 - 0x0014), U16_TO_U8S_LE(0x0004),
    U16_TO_U8S_LE(0x0007), U16_TO_U8S_LE(0x002A),
    'D', 0x00, 'e', 0x00, 'v', 0x00, 'i', 0x00, 'c', 0x00, 'e', 0x00, 'I', 0x00, 'n', 0x00,
    't', 0x00, 'e', 0x00, 'r', 0x00, 'f', 0x00, 'a', 0x00, 'c', 0x00, 'e', 0x00, 'G', 0x00,
    'U', 0x00, 'I', 0x00, 'D', 0x00, 's', 0x00, 0x00, 0x00,
    U16_TO_U8S_LE(0x0050),
    '{', 0x00, '8', 0x00, 'E', 0x00, '9', 0x00, 'A', 0x00, '3', 0x00, 'F', 0x00, '4', 0x00,
    '2', 0x00, '-', 0x00, '5', 0x00, 'C', 0x00, '7', 0x00, '1', 0x00, '-', 0x00, '4', 0x00,
    'B', 0x00, '0', 0x00, 'E', 0x00, '-', 0x00, '9', 0x00, 'D', 0x00, '2', 0x00, '6', 0x00,
    '-', 0x00, '4', 0x00, 'F', 0x00, '1', 0x00, 'A', 0x00, '2', 0x00, 'B', 0x00, '3', 0x00,
    'C', 0x00, '4', 0x00, 'D', 0x00, '5', 0x00, 'E', 0x00, '}', 0x00, 0x00, 0x00, 0x00, 0x00,
};
// clang-format on

TU_VERIFY_STATIC(sizeof(desc_ms_os_20) == MS_OS_20_DESC_LEN, "Incorrect MS OS 2.0 descriptor size");

const uint8_t *tud_descriptor_bos_cb(void) {
    return desc_bos;
}

bool tud_vendor_control_xfer_cb(uint8_t rhport, uint8_t stage, tusb_control_request_t const *request) {
    if (stage != CONTROL_STAGE_SETUP) {
        return true;
    }
    if (request->bmRequestType_bit.type == TUSB_REQ_TYPE_VENDOR && request->bRequest == VENDOR_REQUEST_MICROSOFT &&
        request->wIndex == 7) {
        return tud_control_xfer(rhport, request, (void *)(uintptr_t)desc_ms_os_20, MS_OS_20_DESC_LEN);
    }
    return false;
}

#define CONFIG_TOTAL_LEN                                                                                               \
    (TUD_CONFIG_DESC_LEN + TUD_HID_DESC_LEN + TUD_VENDOR_DESC_LEN + TUD_CDC_DESC_LEN + TUD_HID_DESC_LEN)

// clang-format off
static const uint8_t desc_configuration[] = {
    TUD_CONFIG_DESCRIPTOR(1, ITF_NUM_TOTAL, 0, CONFIG_TOTAL_LEN, 0xA0, 100),
    TUD_HID_DESCRIPTOR(ITF_NUM_HID, 0, HID_ITF_PROTOCOL_KEYBOARD, sizeof(desc_hid_kbd_report), EPNUM_HID, CFG_TUD_HID_EP_BUFSIZE, 1),
    TUD_VENDOR_DESCRIPTOR(ITF_NUM_VENDOR, 0, EPNUM_VENDOR_OUT, EPNUM_VENDOR_IN, FW_USB_PACKET_SIZE),
    TUD_CDC_DESCRIPTOR(ITF_NUM_CDC, 4, EPNUM_CDC_NOTIF, 8, EPNUM_CDC_OUT, EPNUM_CDC_IN, 64),
    TUD_HID_DESCRIPTOR(ITF_NUM_HID_VENDOR, 0, HID_ITF_PROTOCOL_NONE, sizeof(desc_hid_of_report), EPNUM_HID_VENDOR, 8, 1),
};
// clang-format on

const uint8_t *tud_descriptor_configuration_cb(uint8_t index) {
    (void)index;
    return desc_configuration;
}

static char g_serial_str[2 * PICO_UNIQUE_BOARD_ID_SIZE_BYTES + 1] = "openflux-fw-0001";

static const char *string_desc_arr[] = {
    (const char[]){0x09, 0x04}, "OpenFlux", "OpenFlux Firmware", g_serial_str, "OpenFlux Firmware CDC",
};

static uint16_t _desc_str[32];

const uint16_t *tud_descriptor_string_cb(uint8_t index, uint16_t langid) {
    (void)langid;
    uint8_t chr_count;
    if (index == 3) {
        pico_get_unique_board_id_string(g_serial_str, sizeof(g_serial_str));
    }
    if (index == 0) {
        memcpy(&_desc_str[1], string_desc_arr[0], 2);
        chr_count = 1;
    } else {
        if (index >= sizeof(string_desc_arr) / sizeof(string_desc_arr[0]))
            return NULL;
        const char *str = string_desc_arr[index];
        chr_count = (uint8_t)strlen(str);
        if (chr_count > 31)
            chr_count = 31;
        for (uint8_t i = 0; i < chr_count; i++) {
            _desc_str[1 + i] = (uint16_t)str[i];
        }
    }
    _desc_str[0] = (uint16_t)((TUSB_DESC_STRING << 8) | (2 * chr_count + 2));
    return _desc_str;
}

extern uint16_t fw_on_hid_get_report(uint8_t instance, uint8_t report_id, uint8_t *buffer, uint16_t reqlen);
extern void fw_on_hid_led_report(uint8_t led_mask);
extern void fw_on_hid_protocol_changed(uint8_t protocol);

uint16_t tud_hid_get_report_cb(uint8_t instance, uint8_t report_id, hid_report_type_t report_type, uint8_t *buffer,
                               uint16_t reqlen) {
    if (report_type != HID_REPORT_TYPE_INPUT)
        return 0;
    return fw_on_hid_get_report(instance, report_id, buffer, reqlen);
}

void tud_hid_set_protocol_cb(uint8_t instance, uint8_t protocol) {
    if (instance == 0U) {
        fw_on_hid_protocol_changed(protocol);
    }
}

void tud_hid_set_report_cb(uint8_t instance, uint8_t report_id, hid_report_type_t report_type, uint8_t const *buffer,
                           uint16_t bufsize) {
    if (instance == 0U && (report_type == HID_REPORT_TYPE_OUTPUT || report_type == HID_REPORT_TYPE_INVALID) &&
        (report_id == 0U || report_id == 1U) && buffer != NULL && bufsize >= 1U) {
        fw_on_hid_led_report(buffer[0]);
    }
}
