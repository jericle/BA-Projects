import sys

def reverse_string(s):
    return s[::-1]

if __name__ == "__main__":
    if len(sys.argv) > 1:
        input_string = ' '.join(sys.argv[1:])
        print(reverse_string(input_string))
    else:
        print("Usage: python reverse_string.py <string>")